//! A small JSON-only file picker and values-only content index.
//!
//! The layout follows FFF's separation of a stable file table, an immutable
//! bigram index, and a changed-file overlay. Collection paths are flat, so IDs
//! are stored once in a byte arena instead of FFF's general chunked path store.

mod git;
mod regex_plan;
mod typed;
mod view;

use crate::{
    Direction, Error, Field, IndexField, Result,
    error::io,
    json::{ParsedValue, indexed_value_text, value_text},
    query::Operator,
    source::{Source, Stamp},
};
use fff_search::bigram_filter::{BigramFilter, BigramIndexBuilder};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

pub use git::GitState;
use typed::{TypedIndex, TypedKey};
use view::ViewIndex;


/// Stable slot metadata for one source JSON file.
#[derive(Debug, Clone)]
struct FileEntry {
    id_start: u32,
    id_len: u32,
    stamp: Stamp,
    revision: u64,
    changed_at_nanos: u128,
}


/// Wall stages and summed worker time recorded while constructing a picker.
#[derive(Clone, Copy, Default)]
pub(crate) struct BuildTimings {
    pub inventory_nanos: u64,
    pub slots_nanos: u64,
    pub scan_nanos: u64,
    pub project_worker_nanos: u64,
    pub add_worker_nanos: u64,
    pub compress_nanos: u64,
    pub typed_nanos: u64,
    pub view_nanos: u64,
}


/// Stable numeric file slots plus a separate byte-sorted view of live IDs.
/// Changed/new slots bypass the immutable bigram index until it is rebuilt.
pub struct FilePicker {
    root: PathBuf,
    id_bytes: Vec<u8>,
    files: Vec<Option<FileEntry>>,
    sorted: Vec<usize>,
    tombstones: BTreeMap<String, (u64, u128)>,
    indexed_len: usize,
    changed: BTreeSet<usize>,
    array_fields: BTreeSet<String>,
    index: BigramFilter,
    typed: TypedIndex,
    view: ViewIndex,
    build_timings: BuildTimings,
}


impl FilePicker {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_indexes(root, &[], &BTreeMap::new())
    }

    pub(crate) fn open_with_indexes(
        root: &Path,
        indexes: &[IndexField],
        definitions: &BTreeMap<String, Field>,
    ) -> Result<Self> {
        let root = std::fs::canonicalize(root).map_err(|e| io(root, e))?;
        let source = Source::open(&root)?;
        let inventory_started = Instant::now();
        let inventory = source.inventory()?;
        let inventory_nanos = elapsed_nanos(inventory_started);
        let mut indexed_fields = indexes
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        indexed_fields.sort();
        indexed_fields.dedup();
        let mut picker = Self {
            root,
            id_bytes: Vec::new(),
            files: Vec::with_capacity(inventory.len()),
            sorted: Vec::with_capacity(inventory.len()),
            tombstones: BTreeMap::new(),
            indexed_len: 0,
            changed: BTreeSet::new(),
            array_fields: BTreeSet::new(),
            index: BigramIndexBuilder::new(0).compress(None),
            view: ViewIndex::new(&indexed_fields, definitions),
            typed: TypedIndex::new(indexed_fields),
            build_timings: BuildTimings {
                inventory_nanos,
                ..BuildTimings::default()
            },
        };
        let slots_started = Instant::now();
        for (id, stamp) in inventory {
            picker.insert(&id, stamp)?;
        }
        picker.build_timings.slots_nanos = elapsed_nanos(slots_started);
        picker.rebuild_index_from_source(&source)?;
        source.finish()?;
        Ok(picker)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.sorted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    pub(crate) fn sorted_slots(&self) -> &[usize] {
        &self.sorted
    }

    pub(crate) fn view_record(&self, slot: usize) -> (&str, &Stamp) {
        (
            self.id(slot).expect("live view slot"),
            &self.files[slot].as_ref().expect("live view slot").stamp,
        )
    }

    pub(crate) fn view_ordered_slots(
        &self,
        field: &str,
        direction: Direction,
    ) -> Option<Vec<usize>> {
        self.view.ordered_slots(field, direction)
    }

    pub(crate) fn view_filter_bits(
        &self,
        field: &str,
        operator: Operator,
        value: &serde_json::Value,
    ) -> Option<Vec<u64>> {
        self.view
            .filter_bits(field, operator, value, self.files.len())
    }

    pub(crate) fn has_array_value(&self, field: &str) -> bool {
        self.array_fields.contains(field)
    }

    fn id(&self, slot: usize) -> Option<&str> {
        let entry = self.files.get(slot)?.as_ref()?;
        let start = entry.id_start as usize;
        let end = start + entry.id_len as usize;
        std::str::from_utf8(&self.id_bytes[start..end]).ok()
    }

    /// The latest revision, including a deletion tombstone for an absent ID.
    pub fn revision(&self, id: &str) -> Option<u64> {
        self.revision_record(id).map(|(revision, _, _)| revision)
    }

    pub(crate) fn revision_record(&self, id: &str) -> Option<(u64, u128, bool)> {
        self.position(id)
            .ok()
            .and_then(|position| self.files[self.sorted[position]].as_ref())
            .map(|entry| (entry.revision, entry.changed_at_nanos, true))
            .or_else(|| {
                self.tombstones
                    .get(id)
                    .map(|&(revision, changed)| (revision, changed, false))
            })
    }

    pub fn git_status(&self) -> Result<Option<BTreeMap<String, GitState>>> {
        git::status(&self.root)
    }

    /// Refreshes inventory from the authoritative directory. A few edits only
    /// enter the overlay; they never force a full bigram rebuild.
    pub fn refresh(&mut self) -> Result<usize> {
        let source = Source::open(&self.root)?;
        let inventory = source.inventory()?;
        let mut seen = vec![false; self.files.len()];
        let mut changed = 0;
        for (id, stamp) in inventory {
            match self.position(&id) {
                Ok(position) => {
                    let slot = self.sorted[position];
                    seen[slot] = true;
                    if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                        let projection = projected_values(
                            &source,
                            &self.root,
                            &id,
                            &stamp,
                            &[],
                            &self.view,
                            false,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        if let Some(projection) = projection {
                            self.array_fields.extend(projection.array_fields);
                        }
                        let entry = self.files[slot].as_mut().expect("live sorted slot");
                        entry.stamp = stamp;
                        entry.revision += 1;
                        entry.changed_at_nanos = now_nanos();
                        self.changed.insert(slot);
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                        changed += 1;
                    }
                }
                Err(_) => {
                    let projection =
                        projected_values(&source, &self.root, &id, &stamp, &[], &self.view, false)?;
                    let keys = projection
                        .as_ref()
                        .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                    if let Some(projection) = projection {
                        self.array_fields.extend(projection.array_fields);
                    }
                    let slot = self.insert(&id, stamp)?;
                    self.view.update(slot, keys, &self.files, &self.id_bytes);
                    seen.push(true);
                    debug_assert_eq!(slot + 1, seen.len());
                    changed += 1;
                }
            }
        }
        for (slot, present) in seen.iter().enumerate() {
            if *present {
                continue;
            }
            if let Some(entry) = self.files[slot].take() {
                let start = entry.id_start as usize;
                let end = start + entry.id_len as usize;
                let id = std::str::from_utf8(&self.id_bytes[start..end])
                    .expect("validated ID")
                    .to_owned();
                self.tombstones
                    .insert(id, (entry.revision + 1, now_nanos()));
                self.changed.remove(&slot);
                self.view.remove(slot);
                changed += 1;
            }
        }
        self.sorted.retain(|&slot| seen[slot]);
        Ok(changed)
    }

    /// Updates a single path after an application edit or a future watcher
    /// event, without enumerating the directory.
    pub fn refresh_id(&mut self, id: &str) -> Result<()> {
        crate::repository::validate_id(id)?;
        let source = Source::open(&self.root)?;
        let file = match source.file(id) {
            Ok(file) => Some(file),
            Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        match file {
            Some(file) => {
                let path = self.root.join(format!("{id}.json"));
                let stamp =
                    Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                match self.position(id) {
                    Ok(position) => {
                        let slot = self.sorted[position];
                        if self.files[slot].as_ref().expect("live sorted slot").stamp != stamp {
                            let projection = projected_values(
                                &source,
                                &self.root,
                                id,
                                &stamp,
                                &[],
                                &self.view,
                                false,
                            )?;
                            let keys = projection
                                .as_ref()
                                .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                            if let Some(projection) = projection {
                                self.array_fields.extend(projection.array_fields);
                            }
                            let entry = self.files[slot].as_mut().expect("live sorted slot");
                            entry.stamp = stamp;
                            entry.revision += 1;
                            entry.changed_at_nanos = now_nanos();
                            self.changed.insert(slot);
                            self.view.update(slot, keys, &self.files, &self.id_bytes);
                        }
                    }
                    Err(_) => {
                        let projection = projected_values(
                            &source,
                            &self.root,
                            id,
                            &stamp,
                            &[],
                            &self.view,
                            false,
                        )?;
                        let keys = projection
                            .as_ref()
                            .map_or_else(|| self.view.missing_keys(), |p| p.view_keys.clone());
                        if let Some(projection) = projection {
                            self.array_fields.extend(projection.array_fields);
                        }
                        let slot = self.insert(id, stamp)?;
                        self.view.update(slot, keys, &self.files, &self.id_bytes);
                    }
                }
            }
            None => {
                if let Ok(position) = self.position(id) {
                    let slot = self.sorted.remove(position);
                    if let Some(entry) = self.files[slot].take() {
                        self.tombstones
                            .insert(id.to_owned(), (entry.revision + 1, now_nanos()));
                    }
                    self.changed.remove(&slot);
                    self.view.remove(slot);
                }
            }
        }
        source.finish()?;
        Ok(())
    }

    /// Conservative candidates for an OR of necessary literal fragments.
    /// `None` means no safe prefilter; a missing posting also means scan all.
    fn candidates(&self, literals: Option<&[String]>) -> Vec<usize> {
        let Some(literals) = literals.filter(|terms| !terms.is_empty()) else {
            return self.sorted.clone();
        };
        let mut bitsets = Vec::with_capacity(literals.len());
        for literal in literals {
            let Some(bits) = self.index.query(literal.as_bytes()) else {
                return self.sorted.clone();
            };
            bitsets.push(bits);
        }
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                slot >= self.indexed_len
                    || self.changed.contains(&slot)
                    || bitsets
                        .iter()
                        .any(|bits| BigramFilter::is_candidate(bits, slot))
            })
            .collect()
    }

    /// Returns byte-sorted candidate IDs for an OR of necessary literal
    /// fragments. The result must still be parsed and checked against the
    /// complete filter. `None` conservatively returns every file.
    pub fn candidate_ids(&self, literals: Option<&[String]>) -> Vec<&str> {
        self.candidates(literals)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    /// Candidate IDs for a regex over decoded scalar values. FFF's regex HIR
    /// planner narrows the immutable index; changed files bypass it. This is
    /// only a prefilter and may include false positives.
    pub fn regex_candidate_ids(&self, pattern: &str) -> Vec<&str> {
        self.regex_candidate_slots(pattern)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    fn regex_candidate_slots(&self, pattern: &str) -> Vec<usize> {
        let Some(bits) = regex_plan::candidates(&self.index, pattern) else {
            return self.sorted.clone();
        };
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                slot >= self.indexed_len
                    || self.changed.contains(&slot)
                    || BigramFilter::is_candidate(&bits, slot)
            })
            .collect()
    }

    pub(crate) fn candidate_records(
        &self,
        pattern: Option<&str>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
    ) -> Vec<(&str, &Stamp)> {
        let bits = pattern.and_then(|regex| regex_plan::candidates(&self.index, regex));
        let typed_bits = self
            .typed
            .candidate_bits(typed_plan, self.files.len(), &self.changed);
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                let text_match = bits.as_ref().is_none_or(|bits| {
                    slot >= self.indexed_len
                        || self.changed.contains(&slot)
                        || BigramFilter::is_candidate(bits, slot)
                });
                let typed_match = typed_bits
                    .as_ref()
                    .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0);
                text_match && typed_match
            })
            .map(|slot| {
                (
                    self.id(slot).expect("live candidate slot"),
                    &self.files[slot].as_ref().expect("live candidate").stamp,
                )
            })
            .collect()
    }

    /// Complete regex search over the values projection. Source JSON is
    /// reopened and parsed for each candidate; the index never decides truth.
    pub fn search_values_regex(&self, pattern: &str) -> Result<Vec<String>> {
        let matcher = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .map_err(|e| Error::Request(format!("invalid values regex: {e}")))?;
        let source = Source::open(&self.root)?;
        let candidates = self.regex_candidate_ids(pattern);
        let matches = candidates
            .par_iter()
            .map(|&id| -> Result<Option<String>> {
                let slot = self.sorted[self.position(id).expect("candidate is live")];
                let stamp = &self.files[slot].as_ref().expect("live candidate").stamp;
                let text =
                    projected_values(&source, &self.root, id, stamp, &[], &self.view, false)?;
                Ok(text
                    .as_ref()
                    .is_some_and(|text| matcher.is_match(&text.bytes))
                    .then(|| id.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        source.finish()?;
        Ok(matches.into_iter().flatten().collect())
    }

    /// Rebuilds the immutable index after many edits. Source stamps are checked
    /// before and after every read so a racing edit cannot publish a bad index.
    pub fn rebuild_index(&mut self) -> Result<()> {
        let source = Source::open(&self.root)?;
        self.rebuild_index_from_source(&source)
    }

    fn rebuild_index_from_source(&mut self, source: &Source) -> Result<()> {
        // Compact removed IDs while rebuilding; changed slots remain stable
        // between rebuilds and can be reassigned here with the fresh index.
        let slots_started = Instant::now();
        let mut ids = Vec::with_capacity(self.id_bytes.len());
        let mut files = Vec::with_capacity(self.sorted.len());
        for &slot in &self.sorted {
            let mut entry = self.files[slot].as_ref().expect("live sorted slot").clone();
            let id = self.id(slot).expect("validated ID");
            entry.id_start = u32::try_from(ids.len())
                .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
            ids.extend_from_slice(id.as_bytes());
            files.push(Some(entry));
        }
        let main = BigramIndexBuilder::new(files.len());
        let skip = BigramIndexBuilder::new(files.len());
        let array_fields = std::sync::Mutex::new(BTreeSet::new());
        let indexed_fields = self.typed.fields();
        let typed_rows = std::sync::Mutex::new(vec![Vec::new(); indexed_fields.len()]);
        let view_rows = std::sync::Mutex::new(Vec::new());
        let view = &self.view;
        let root = &self.root;
        self.build_timings.slots_nanos += elapsed_nanos(slots_started);
        let project_worker = AtomicU64::new(0);
        let add_worker = AtomicU64::new(0);
        let scan_started = Instant::now();
        files
            .par_chunks(64)
            .enumerate()
            .try_for_each(|(chunk_index, chunk)| -> Result<()> {
                let mut chunk_arrays = BTreeSet::new();
                let mut chunk_typed = vec![Vec::new(); indexed_fields.len()];
                let mut chunk_view = Vec::new();
                let mut chunk_project_nanos = 0;
                let mut chunk_add_nanos = 0;
                for (within, entry) in chunk.iter().enumerate() {
                    let Some(entry) = entry else { continue };
                    let id = std::str::from_utf8(
                        &ids[entry.id_start as usize..(entry.id_start + entry.id_len) as usize],
                    )
                    .expect("validated UTF-8 ID");
                    let project_started = Instant::now();
                    let projection = projected_values(
                        source,
                        root,
                        id,
                        &entry.stamp,
                        indexed_fields,
                        view,
                        true,
                    )?;
                    chunk_project_nanos += elapsed_nanos(project_started);
                    if let Some(projection) = projection {
                        let slot = chunk_index * 64 + within;
                        let add_started = Instant::now();
                        main.add_file_content(&skip, slot, &projection.bytes);
                        chunk_add_nanos += elapsed_nanos(add_started);
                        chunk_arrays.extend(projection.array_fields);
                        chunk_view.push((slot, projection.view_keys));
                        for (field, key) in projection.typed_values {
                            chunk_typed[field].push((key, slot));
                        }
                    }
                }
                array_fields
                    .lock()
                    .expect("array field lock")
                    .extend(chunk_arrays);
                project_worker.fetch_add(chunk_project_nanos, Ordering::Relaxed);
                add_worker.fetch_add(chunk_add_nanos, Ordering::Relaxed);
                let mut rows = typed_rows.lock().expect("typed row lock");
                for (all, chunk) in rows.iter_mut().zip(chunk_typed) {
                    all.extend(chunk);
                }
                view_rows.lock().expect("view row lock").extend(chunk_view);
                Ok(())
            })?;
        self.build_timings.scan_nanos = elapsed_nanos(scan_started);
        self.build_timings.project_worker_nanos = project_worker.load(Ordering::Relaxed);
        self.build_timings.add_worker_nanos = add_worker.load(Ordering::Relaxed);
        let compress_started = Instant::now();
        let mut index = main.compress(None);
        index.set_skip_index(skip.compress(Some(12))); // FFF's current skip-index cutoff.
        self.build_timings.compress_nanos = elapsed_nanos(compress_started);
        source.finish()?;
        self.id_bytes = ids;
        self.files = files;
        self.sorted = (0..self.files.len()).collect();
        self.index = index;
        self.indexed_len = self.files.len();
        self.changed.clear();
        self.array_fields = array_fields.into_inner().expect("array field lock");
        let typed_started = Instant::now();
        self.typed
            .replace_rows(typed_rows.into_inner().expect("typed row lock"));
        self.build_timings.typed_nanos = elapsed_nanos(typed_started);
        let view_started = Instant::now();
        self.view.replace(
            view_rows.into_inner().expect("view row lock"),
            &self.files,
            &self.id_bytes,
        );
        self.build_timings.view_nanos = elapsed_nanos(view_started);
        Ok(())
    }

    pub fn index_bytes(&self) -> usize {
        self.index.heap_bytes()
    }

    pub(crate) fn build_timings(&self) -> BuildTimings {
        self.build_timings
    }

    fn position(&self, id: &str) -> std::result::Result<usize, usize> {
        self.sorted.binary_search_by(|&slot| {
            self.id(slot)
                .expect("live sorted slot")
                .as_bytes()
                .cmp(id.as_bytes())
        })
    }

    fn insert(&mut self, id: &str, stamp: Stamp) -> Result<usize> {
        let start = u32::try_from(self.id_bytes.len())
            .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        let len = u32::try_from(id.len())
            .map_err(|_| Error::Cache("file picker ID exceeds 4 GiB".into()))?;
        let _ = start
            .checked_add(len)
            .ok_or_else(|| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
        let position = self.position(id).expect_err("inserted ID must be new");
        self.id_bytes.extend_from_slice(id.as_bytes());
        let slot = self.files.len();
        self.files.push(Some(FileEntry {
            id_start: start,
            id_len: len,
            stamp,
            revision: self
                .tombstones
                .remove(id)
                .map_or(1, |(revision, _)| revision + 1),
            changed_at_nanos: now_nanos(),
        }));
        self.sorted.insert(position, slot);
        self.changed.insert(slot);
        Ok(slot)
    }
}


fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos())
}

fn elapsed_nanos(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}


/// Search text and typed values extracted from one source document.
struct Projection {
    bytes: Vec<u8>,
    array_fields: Vec<String>,
    typed_values: Vec<(usize, TypedKey)>,
    view_keys: Vec<crate::query::Scalar>,
}


fn projected_values(
    source: &Source,
    root: &Path,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    view: &ViewIndex,
    folded_index: bool,
) -> Result<Option<Projection>> {
    let path = root.join(format!("{id}.json"));
    let mut file = source.file(id)?;
    let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if &before != stamp {
        return Err(Error::Unstable(path));
    }
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).map_err(|e| io(&path, e))?;
    let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if after != before {
        return Err(Error::Unstable(path));
    }
    let mut values = String::new();
    let Ok(parsed) = sonic_rs::from_slice::<ParsedValue>(&raw) else {
        return Ok(None);
    };
    if folded_index {
        indexed_value_text(&parsed.0, &mut values);
    } else {
        value_text(&parsed.0, &mut values);
    }
    let array_fields = parsed.0.as_object().map_or_else(Vec::new, |fields| {
        fields
            .iter()
            .filter(|(_, value)| value.is_array())
            .map(|(key, _)| key.clone())
            .collect()
    });
    let typed_values = parsed.0.as_object().map_or_else(Vec::new, |fields| {
        indexed_fields
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                fields
                    .get(name)
                    .and_then(TypedKey::from_value)
                    .map(|key| (index, key))
            })
            .collect()
    });
    Ok(Some(Projection {
        bytes: values.into_bytes(),
        array_fields,
        typed_values,
        view_keys: view.project(id, &parsed.0),
    }))
}

#[cfg(test)]
#[path = "test.rs"]
mod tests;
