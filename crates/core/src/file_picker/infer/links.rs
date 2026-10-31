//! Resolve possible record IDs and JSON Pointer fragments against real files.

use crate::{
    InferredLink, InferredLinkKind, RepositoryModel, Result, config::fingerprint, source::Source,
};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};


/// Repository-wide IDs and paths collected without parsing source JSON.
pub(crate) struct ReferenceInventory {
    root: PathBuf,
    ids: HashMap<String, Vec<String>>,
    paths: HashMap<PathBuf, String>,
    snapshot: String,
}


impl ReferenceInventory {
    pub(crate) fn build(model: &RepositoryModel) -> Result<Self> {
        let mut ids = HashMap::<String, Vec<String>>::new();
        let mut paths = HashMap::new();
        let mut snapshots = Vec::new();
        for collection in model.collections.values() {
            let source = Source::open_scoped(&collection.path, collection.recursive)?;
            let inventory = source.inventory()?;
            snapshots.push((
                collection.name.clone(),
                fingerprint("reference-source/v1", &inventory),
            ));
            for (id, _) in inventory {
                ids.entry(id.clone())
                    .or_default()
                    .push(collection.name.clone());
                paths.insert(
                    collection.path.join(format!("{id}.json")),
                    collection.name.clone(),
                );
            }
            source.finish()?;
        }
        Ok(Self {
            root: model.root.clone(),
            ids,
            paths,
            snapshot: fingerprint("reference-inventory/v1", &snapshots),
        })
    }

    pub(crate) fn snapshot(&self) -> &str {
        &self.snapshot
    }

    fn resolve(
        &self,
        value: &str,
        source_file: &Path,
        raw: &[u8],
    ) -> Option<(InferredLinkKind, String)> {
        if let Some(collections) = self.ids.get(value)
            && let [collection] = collections.as_slice()
        {
            return Some((InferredLinkKind::RecordId, collection.clone()));
        }
        let (file, fragment) = value.split_once('#')?;
        if !file.is_empty() && !file.ends_with(".json") {
            return None;
        }
        let pointer = decode_fragment(fragment)?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return None;
        }
        let target_file = if file.is_empty() {
            source_file.to_owned()
        } else {
            let from_source = normalize(source_file.parent()?.join(file));
            if self.paths.contains_key(&from_source) {
                from_source
            } else {
                normalize(self.root.join(file))
            }
        };
        let target_collection = self.paths.get(&target_file)?.clone();
        let target = if target_file == source_file {
            raw.to_vec()
        } else {
            fs::read(&target_file).ok()?
        };
        let document: serde_json::Value = serde_json::from_slice(&target).ok()?;
        if pointer.is_empty() || document.pointer(&pointer).is_some() {
            Some((InferredLinkKind::JsonPointer, target_collection))
        } else {
            None
        }
    }
}


/// Bounded semantic evidence: values are checked, but never retained.
#[derive(Default)]
pub(super) struct LinkVotes {
    values: usize,
    resolved: HashMap<(InferredLinkKind, String), usize>,
}


impl LinkVotes {
    pub(super) fn observe(
        &mut self,
        value: &str,
        inventory: &ReferenceInventory,
        source_file: &Path,
        raw: &[u8],
    ) {
        if value.is_empty() {
            return;
        }
        self.values += 1;
        if let Some(target) = inventory.resolve(value, source_file, raw) {
            *self.resolved.entry(target).or_default() += 1;
        }
    }

    pub(super) fn inferred(&self, source_records: usize) -> Option<InferredLink> {
        if self.values < 10 || source_records < 5 {
            return None;
        }
        let ((kind, target_collection), &resolved_values) =
            self.resolved.iter().max_by_key(|(_, count)| *count)?;
        if resolved_values * 100 < self.values * 95 {
            return None;
        }
        Some(InferredLink {
            kind: *kind,
            target_collection: target_collection.clone(),
            resolved_values,
            total_values: self.values,
        })
    }
}


fn decode_fragment(fragment: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}
