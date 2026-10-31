//! Caller-built ordered postings for one or several JSON paths.

use super::*;
use crate::{JsonPath, json::ParsedValue, json_filter::values};
use std::{io::Read, sync::Mutex};


/// Accumulated rows and presence bits for one path across all worker chunks.
struct PostingRows {
    rows: Vec<(TypedKey, usize)>,
    present: Vec<u64>,
    scalar_safe: bool,
    complete_values: bool,
}


impl PostingRows {
    fn new(slots: usize) -> Self {
        Self {
            rows: Vec::new(),
            present: vec![0; slots.div_ceil(64)],
            scalar_safe: true,
            complete_values: true,
        }
    }
}


/// Small per-chunk rows merged once rather than locking for every file.
struct ChunkRows {
    rows: Vec<(TypedKey, usize)>,
    present: Vec<usize>,
    scalar_safe: bool,
    complete_values: bool,
}


impl ChunkRows {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            present: Vec::new(),
            scalar_safe: true,
            complete_values: true,
        }
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
        let entries = self
            .sorted
            .iter()
            .map(|&slot| {
                (
                    slot,
                    self.id(slot).expect("live posting slot").to_owned(),
                    self.files[slot]
                        .as_ref()
                        .expect("live posting slot")
                        .stamp
                        .clone(),
                )
            })
            .collect::<Vec<_>>();
        let builders = paths
            .iter()
            .map(|_| Mutex::new(PostingRows::new(self.files.len())))
            .collect::<Vec<_>>();
        scan_pool::run(|| {
            entries.par_chunks(64).try_for_each(|chunk| -> Result<()> {
                let mut local = paths.iter().map(|_| ChunkRows::new()).collect::<Vec<_>>();
                for (slot, id, stamp) in chunk {
                    let file_path = source.path.join(format!("{id}.json"));
                    let mut file = source.file(id)?;
                    let before = Stamp::from_metadata(
                        &file.metadata().map_err(|error| io(&file_path, error))?,
                        &file_path,
                    )?;
                    if &before != stamp {
                        return Err(Error::Unstable(file_path));
                    }
                    let mut raw = Vec::new();
                    file.read_to_end(&mut raw)
                        .map_err(|error| io(&file_path, error))?;
                    let after = Stamp::from_metadata(
                        &file.metadata().map_err(|error| io(&file_path, error))?,
                        &file_path,
                    )?;
                    if before != after || raw.len() as u64 != after.size {
                        return Err(Error::Unstable(file_path));
                    }
                    let Ok(ParsedValue(document)) = sonic_rs::from_slice::<ParsedValue>(&raw)
                    else {
                        continue;
                    };
                    for (path, posting) in paths.iter().zip(local.iter_mut()) {
                        let present = !values(&document, path, false).is_empty();
                        if present {
                            posting.present.push(*slot);
                        }
                        let selected = values(&document, path, true);
                        posting.scalar_safe &= selected.len() <= 1;
                        if !present {
                            posting.rows.push((TypedKey::Missing, *slot));
                        } else {
                            for value in selected {
                                if let Some(key) = TypedKey::from_value(value) {
                                    posting.rows.push((key, *slot));
                                } else {
                                    posting.complete_values = false;
                                }
                            }
                        }
                    }
                }
                for (builder, chunk) in builders.iter().zip(local) {
                    let mut builder = builder.lock().expect("posting row lock");
                    builder.rows.extend(chunk.rows);
                    for slot in chunk.present {
                        builder.present[slot / 64] |= 1 << (slot % 64);
                    }
                    builder.scalar_safe &= chunk.scalar_safe;
                    builder.complete_values &= chunk.complete_values;
                }
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
            );
            self.inferred_paths.remove(&path);
        }
        Ok(())
    }
}
