//! Caller-built ordered postings for a JSON path.

use super::*;
use crate::{JsonPath, json::ParsedValue, json_filter::values};
use std::io::Read;


impl FilePicker {
    /// Replace one path's posting from the current source snapshot.
    pub(crate) fn build_posting(&mut self, path: &JsonPath) -> Result<()> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let entries = self
            .sorted
            .iter()
            .map(|&slot| {
                let id = self.id(slot).expect("live posting slot").to_owned();
                let stamp = self.files[slot]
                    .as_ref()
                    .expect("live posting slot")
                    .stamp
                    .clone();
                (slot, id, stamp)
            })
            .collect::<Vec<_>>();
        let projected = scan_pool::run(|| {
            entries
                .par_iter()
                .map(|(slot, id, stamp)| {
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
                    let Ok(ParsedValue(doc)) = sonic_rs::from_slice::<ParsedValue>(&raw) else {
                        return Ok((*slot, None, false, true, false));
                    };
                    let present = !values(&doc, path, false).is_empty();
                    let selected = values(&doc, path, true);
                    let scalar_safe = selected.len() <= 1;
                    let key = if !present {
                        Some(TypedKey::Missing)
                    } else if scalar_safe {
                        selected
                            .first()
                            .and_then(|value| TypedKey::from_value(value))
                    } else {
                        None
                    };
                    Ok((*slot, key, present, scalar_safe, true))
                })
                .collect::<Result<Vec<_>>>()
        })?;
        source.finish()?;
        let mut rows = Vec::new();
        let mut present = vec![0u64; self.files.len().div_ceil(64)];
        let mut scalar_safe = true;
        for (slot, key, exists, safe, valid) in projected {
            if !valid {
                continue;
            }
            if exists {
                present[slot / 64] |= 1 << (slot % 64);
            }
            scalar_safe &= safe;
            if let Some(key) = key {
                rows.push((key, slot));
            }
        }
        self.typed
            .replace_path(path.clone(), rows, present, scalar_safe);
        Ok(())
    }
}
