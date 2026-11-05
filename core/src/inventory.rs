//! Repository-wide files and collection-generation record ownership.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::model::{CollectionId, FileId, IdentitySet, RecordId, RecordSlot};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRevision {
    pub modified_ns: i128,
    pub size: u64,
}

impl SourceRevision {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, String> {
        let modified = metadata.modified().map_err(|e| e.to_string())?;
        let duration = modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| format!("modification time before Unix epoch: {e}"))?;
        Ok(Self {
            modified_ns: duration.as_nanos() as i128,
            size: metadata.len(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: FileId,
    pub relative_path: PathBuf,
    pub revision: SourceRevision,
    pub created_ns: Option<i128>,
    pub collections: Vec<CollectionId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileInventory {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    #[serde(skip)]
    by_path: HashMap<PathBuf, FileId>,
}

impl FileInventory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: Vec::new(),
            by_path: HashMap::new(),
        }
    }

    pub fn add(
        &mut self,
        relative_path: PathBuf,
        revision: SourceRevision,
        created_ns: Option<i128>,
        collection: CollectionId,
    ) -> Result<FileId, String> {
        if !valid_relative_path(&relative_path) {
            return Err("file path must be repository-relative and contained".into());
        }
        if let Some(&id) = self.by_path.get(&relative_path) {
            let file = &mut self.files[id.0 as usize];
            if file.revision != revision {
                return Err("one file has inconsistent revisions".into());
            }
            if !file.collections.contains(&collection) {
                file.collections.push(collection);
            }
            return Ok(id);
        }
        let id = FileId(u32::try_from(self.files.len()).map_err(|_| "too many files")?);
        self.by_path.insert(relative_path.clone(), id);
        self.files.push(FileEntry {
            id,
            relative_path,
            revision,
            created_ns,
            collections: vec![collection],
        });
        Ok(id)
    }

    pub fn rebuild_lookup(&mut self) -> Result<(), String> {
        self.by_path.clear();
        for (slot, file) in self.files.iter().enumerate() {
            if !valid_relative_path(&file.relative_path)
                || file.id.0 as usize != slot
                || self
                    .by_path
                    .insert(file.relative_path.clone(), file.id)
                    .is_some()
            {
                return Err("invalid restored file inventory".into());
            }
        }
        Ok(())
    }

    pub fn native_path(&self, id: FileId) -> Option<PathBuf> {
        self.files
            .get(id.0 as usize)
            .map(|f| self.root.join(&f.relative_path))
    }

    pub fn id_for(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}
impl ByteRange {
    pub fn checked(start: u64, end: u64, file_size: u64) -> Result<Self, String> {
        if start > end || end > file_size {
            return Err(format!(
                "invalid byte range {start}..{end} for file size {file_size}"
            ));
        }
        Ok(Self { start, end })
    }
    pub fn len(self) -> u64 {
        self.end - self.start
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordEntry {
    pub slot: RecordSlot,
    pub id: RecordId,
    pub file: FileId,
    pub range: ByteRange,
    pub revision: SourceRevision,
}

/// Compact per-file offset table. `slots[offsets[f]..offsets[f+1]]` belongs
/// exactly to file `f`; empty files have equal consecutive offsets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecordIndex {
    pub offsets: Vec<u32>,
    pub slots: Vec<RecordSlot>,
}
impl FileRecordIndex {
    pub fn records_for(&self, file: FileId) -> Option<&[RecordSlot]> {
        let a = *self.offsets.get(file.0 as usize)? as usize;
        let b = *self.offsets.get(file.0 as usize + 1)? as usize;
        self.slots.get(a..b)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionGeneration {
    pub collection: CollectionId,
    pub name: String,
    pub generation: u64,
    pub files: Vec<FileId>,
    pub records: Vec<RecordEntry>,
    pub file_records: FileRecordIndex,
}

impl CollectionGeneration {
    pub fn validate(&self, inventory: &FileInventory) -> Result<(), String> {
        let mut included = std::collections::HashSet::new();
        for file_id in &self.files {
            if !included.insert(*file_id)
                || inventory
                    .files
                    .get(file_id.0 as usize)
                    .is_none_or(|file| !file.collections.contains(&self.collection))
            {
                return Err("invalid collection file membership".into());
            }
        }
        if self.file_records.offsets.len() != inventory.files.len() + 1
            || self.file_records.offsets.first() != Some(&0)
            || self.file_records.offsets.last().copied()
                != Some(self.file_records.slots.len() as u32)
        {
            return Err("invalid file-record offset table".into());
        }
        let mut seen = vec![false; self.records.len()];
        for (file_idx, window) in self.file_records.offsets.windows(2).enumerate() {
            let (a, b) = (window[0] as usize, window[1] as usize);
            if a > b || b > self.file_records.slots.len() {
                return Err("file-record offsets out of bounds".into());
            }
            for slot in &self.file_records.slots[a..b] {
                let i = slot.0 as usize;
                let entry = self
                    .records
                    .get(i)
                    .ok_or("file-record slot out of bounds")?;
                if seen[i]
                    || entry.slot != *slot
                    || entry.file.0 as usize != file_idx
                    || !included.contains(&entry.file)
                {
                    return Err("file-record ownership mismatch".into());
                }
                let file = &inventory.files[file_idx];
                if !file.collections.contains(&self.collection)
                    || entry.revision != file.revision
                    || entry.range.start >= entry.range.end
                    || entry.range.end > file.revision.size
                {
                    return Err("record range or revision mismatch".into());
                }
                seen[i] = true;
            }
        }
        if seen.iter().any(|v| !v) {
            return Err("unmapped record slot".into());
        }
        Ok(())
    }
}

pub struct CollectionBuilder {
    collection: CollectionId,
    name: String,
    generation: u64,
    files: Vec<FileId>,
    records: Vec<RecordEntry>,
    by_file: Vec<Vec<RecordSlot>>,
    ids: IdentitySet,
}

impl CollectionBuilder {
    pub fn record(&self, slot: RecordSlot) -> Option<&RecordEntry> {
        self.records.get(slot.0 as usize)
    }
    pub fn new(collection: CollectionId, name: String, generation: u64, file_count: usize) -> Self {
        Self {
            collection,
            name,
            generation,
            files: Vec::new(),
            records: Vec::new(),
            by_file: vec![Vec::new(); file_count],
            ids: IdentitySet::default(),
        }
    }
    pub fn include_file(&mut self, file: FileId) -> Result<(), String> {
        if self.by_file.get(file.0 as usize).is_none() {
            return Err("file ID out of bounds".into());
        }
        if !self.files.contains(&file) {
            self.files.push(file);
        }
        Ok(())
    }
    pub fn add_record(
        &mut self,
        file: &FileEntry,
        id: RecordId,
        start: u64,
        end: u64,
    ) -> Result<RecordSlot, String> {
        if !self.files.contains(&file.id) {
            return Err("file is not in collection".into());
        }
        let range = ByteRange::checked(start, end, file.revision.size)?;
        if range.start == range.end {
            return Err("record range must be nonempty".into());
        }
        self.ids.insert(id.clone())?;
        let slot = RecordSlot(u32::try_from(self.records.len()).map_err(|_| "too many records")?);
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
        });
        self.by_file[file.id.0 as usize].push(slot);
        Ok(slot)
    }
    pub fn finish(self, inventory: &FileInventory) -> Result<CollectionGeneration, String> {
        let mut offsets = Vec::with_capacity(self.by_file.len() + 1);
        let mut slots = Vec::with_capacity(self.records.len());
        offsets.push(0);
        for group in self.by_file {
            slots.extend(group);
            offsets.push(u32::try_from(slots.len()).map_err(|_| "too many records")?);
        }
        let result = CollectionGeneration {
            collection: self.collection,
            name: self.name,
            generation: self.generation,
            files: self.files,
            records: self.records,
            file_records: FileRecordIndex { offsets, slots },
        };
        result.validate(inventory)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_record_mapping_in_both_directions() {
        let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
        let revision = SourceRevision {
            modified_ns: 5,
            size: 10,
        };
        let f0 = files
            .add("empty.json".into(), revision, None, CollectionId(0))
            .unwrap();
        let f1 = files
            .add("multi.jsonl".into(), revision, None, CollectionId(0))
            .unwrap();
        assert_eq!(
            files
                .add("multi.jsonl".into(), revision, None, CollectionId(1))
                .unwrap(),
            f1
        );
        let mut a = CollectionBuilder::new(CollectionId(0), "a".into(), 1, files.files.len());
        a.include_file(f0).unwrap();
        a.include_file(f1).unwrap();
        a.add_record(&files.files[1], RecordId::intrinsic("one").unwrap(), 0, 3)
            .unwrap();
        a.add_record(&files.files[1], RecordId::intrinsic("two").unwrap(), 4, 10)
            .unwrap();
        let generation = a.finish(&files).unwrap();
        assert_eq!(generation.file_records.records_for(f0).unwrap().len(), 0);
        assert_eq!(
            generation.file_records.records_for(f1).unwrap(),
            &[RecordSlot(0), RecordSlot(1)]
        );
        assert_eq!(generation.records[1].range, ByteRange { start: 4, end: 10 });
        let mut b = CollectionBuilder::new(CollectionId(1), "b".into(), 1, files.files.len());
        b.include_file(f1).unwrap();
        b.add_record(
            &files.files[1],
            RecordId::intrinsic("other").unwrap(),
            0,
            10,
        )
        .unwrap();
        assert_eq!(
            b.finish(&files)
                .unwrap()
                .file_records
                .records_for(f1)
                .unwrap(),
            &[RecordSlot(0)]
        );
        assert!(ByteRange::checked(9, 11, 10).is_err());
        assert!(ByteRange::checked(5, 4, 10).is_err());
    }
}
