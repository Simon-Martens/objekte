//! Caller-built ordered postings for one or several JSON paths.

use super::*;
use crate::{JsonPath, JsonPathStep, json_filter::sonic_values};
use sonic_rs::Value as SonicValue;
use std::{io::Read, sync::Mutex};


/// Accumulated rows and presence bits for one path across all worker chunks.
pub(super) struct PostingRows {
    pub(super) rows: Vec<(TypedKey, usize)>,
    pub(super) present: Vec<u64>,
    pub(super) scalar_safe: bool,
    pub(super) complete_values: bool,
    pub(super) implicit_equivalent: bool,
}


impl PostingRows {
    pub(super) fn new(slots: usize) -> Self {
        Self {
            rows: Vec::new(),
            present: vec![0; slots.div_ceil(64)],
            scalar_safe: true,
            complete_values: true,
            implicit_equivalent: true,
        }
    }
}


/// Small per-chunk rows merged once rather than locking for every file.
pub(super) struct ChunkRows {
    rows: Vec<(TypedKey, usize)>,
    present: Vec<usize>,
    scalar_safe: bool,
    complete_values: bool,
    implicit_equivalent: bool,
}


impl ChunkRows {
    /// Values of the single document this chunk projected.
    pub(super) fn into_path_values(self) -> super::typed::PathValues {
        super::typed::PathValues {
            keys: self.rows.into_iter().map(|(key, _)| key).collect(),
            present: !self.present.is_empty(),
            scalar_safe: self.scalar_safe,
            complete_values: self.complete_values,
            implicit_equivalent: self.implicit_equivalent,
        }
    }

    pub(super) fn new() -> Self {
        Self {
            rows: Vec::new(),
            present: Vec::new(),
            scalar_safe: true,
            complete_values: true,
            implicit_equivalent: true,
        }
    }
}


pub(super) fn project_document(
    document: &SonicValue,
    slot: usize,
    paths: &[JsonPath],
    rows: &mut [ChunkRows],
) {
    for (path, posting) in paths.iter().zip(rows) {
        let present = !sonic_values(document, path, false).is_empty();
        if present {
            posting.present.push(slot);
        }
        let selected = sonic_values(document, path, true);
        posting.scalar_safe &= selected.len() <= 1;
        // `tags[*]` and the implicit spelling `tags` reach the same values
        // unless a non-array stands where `[*]` expects one.
        if path.iter().any(|step| matches!(step, JsonPathStep::Each)) {
            let implicit = path
                .iter()
                .filter(|step| !matches!(step, JsonPathStep::Each))
                .cloned()
                .collect::<Vec<_>>();
            let natural = sonic_values(document, &implicit, true);
            posting.implicit_equivalent &= natural.len() == selected.len()
                && natural
                    .iter()
                    .zip(&selected)
                    .all(|(a, b)| std::ptr::eq(*a, *b));
        }
        if !present {
            posting.rows.push((TypedKey::Missing, slot));
        } else {
            for value in selected {
                if let Some(key) = TypedKey::from_sonic(value) {
                    posting.rows.push((key, slot));
                } else {
                    posting.complete_values = false;
                }
            }
        }
    }
}

pub(super) fn merge_chunk(builders: &[Mutex<PostingRows>], chunks: Vec<ChunkRows>) {
    for (builder, chunk) in builders.iter().zip(chunks) {
        let mut builder = builder.lock().expect("posting row lock");
        builder.rows.extend(chunk.rows);
        for slot in chunk.present {
            builder.present[slot / 64] |= 1 << (slot % 64);
        }
        builder.scalar_safe &= chunk.scalar_safe;
        builder.complete_values &= chunk.complete_values;
        builder.implicit_equivalent &= chunk.implicit_equivalent;
    }
}


impl FilePicker {
    /// Replace one path's posting from the current source snapshot.
    pub(crate) fn build_posting(&mut self, path: &JsonPath) -> Result<()> {
        self.build_postings(std::slice::from_ref(path))
    }

    /// Read and parse each file once for all requested ordered postings.
    pub(crate) fn build_postings(&mut self, paths: &[JsonPath]) -> Result<()> {
        let paths = paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(());
        }
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let builders = paths
            .iter()
            .map(|_| Mutex::new(PostingRows::new(self.files.len())))
            .collect::<Vec<_>>();
        scan_pool::run(|| {
            self.sorted
                .par_chunks(64)
                .try_for_each(|chunk| -> Result<()> {
                    let mut local = paths.iter().map(|_| ChunkRows::new()).collect::<Vec<_>>();
                    let mut raw = Vec::new();
                    for &slot in chunk {
                        let id = self.id(slot).expect("live posting slot");
                        let stamp = &self.files[slot].as_ref().expect("live posting slot").stamp;
                        let file_path = source.path.join(format!("{id}.json"));
                        let mut file = source.file(id)?;
                        let before = Stamp::from_metadata(
                            &file.metadata().map_err(|error| io(&file_path, error))?,
                            &file_path,
                        )?;
                        if &before != stamp {
                            return Err(Error::Unstable(file_path));
                        }
                        raw.clear();
                        file.read_to_end(&mut raw)
                            .map_err(|error| io(&file_path, error))?;
                        let after = Stamp::from_metadata(
                            &file.metadata().map_err(|error| io(&file_path, error))?,
                            &file_path,
                        )?;
                        if before != after || raw.len() as u64 != after.size {
                            return Err(Error::Unstable(file_path));
                        }
                        let Ok(document) = sonic_rs::from_slice::<SonicValue>(&raw) else {
                            continue;
                        };
                        if !super::projection::valid_document(&document) {
                            continue;
                        }
                        project_document(&document, slot, &paths, &mut local);
                    }
                    merge_chunk(&builders, local);
                    Ok(())
                })
        })?;
        source.finish()?;
        for (path, builder) in paths.into_iter().zip(builders) {
            let builder = builder.into_inner().expect("posting row lock");
            self.typed.replace_path(
                path.clone(),
                builder.rows,
                builder.present,
                builder.scalar_safe,
                builder.complete_values,
                builder.implicit_equivalent,
            );
            self.inferred_paths.remove(&path);
        }
        Ok(())
    }
}
