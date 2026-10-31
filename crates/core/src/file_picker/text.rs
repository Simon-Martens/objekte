//! Lazy bigram postings for every included JSON leaf value.

use super::*;
use crate::text::{render_text, visit_sonic_text};
use sonic_rs::Value as SonicValue;
use std::io::Read;
use unicode_casefold::UnicodeCaseFold;


/// Separate postings over rendered visible text and decoded source markup.
pub(super) struct TextIndex {
    clean: BigramFilter,
    raw: BigramFilter,
    indexed_len: usize,
}


impl TextIndex {
    pub(super) fn heap_bytes(&self) -> usize {
        self.clean.heap_bytes() + self.raw.heap_bytes()
    }
}


impl FilePicker {
    pub(crate) fn text_index_bytes(&self) -> Option<usize> {
        self.text_index.as_ref().map(TextIndex::heap_bytes)
    }

    /// Build full-text postings only when a caller asks for them.
    pub(crate) fn ensure_text_index(&mut self) -> Result<bool> {
        if self.text_index.is_some() {
            return Ok(false);
        }
        let source = Source::open_scoped(&self.root, self.recursive)?;
        self.build_text_index_from_source(&source)?;
        source.finish()?;
        Ok(true)
    }

    pub(crate) fn rebuild_text_index(&mut self) -> Result<()> {
        self.refresh()?;
        self.text_index = None;
        self.ensure_text_index()?;
        Ok(())
    }

    pub(super) fn build_text_index_from_source(&mut self, source: &Source) -> Result<()> {
        let clean_main = BigramIndexBuilder::new(self.files.len());
        let clean_skip = BigramIndexBuilder::new(self.files.len());
        let raw_main = BigramIndexBuilder::new(self.files.len());
        let raw_skip = BigramIndexBuilder::new(self.files.len());
        // FFF writes each posting word with a plain OR. A worker must own
        // all 64 slots in that word or simultaneous writes can lose bits.
        self.files.par_chunks(64).enumerate().try_for_each(
            |(chunk_index, entries)| -> Result<()> {
                for (within, entry) in entries.iter().enumerate() {
                    let Some(entry) = entry else { continue };
                    let slot = chunk_index * 64 + within;
                    let id = self.id(slot).expect("live text-index slot");
                    let path = source.path.join(format!("{id}.json"));
                    let mut file = source.file(id)?;
                    let before =
                        Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                    if before != entry.stamp {
                        return Err(Error::Unstable(path));
                    }
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
                    let after =
                        Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                    if before != after || bytes.len() as u64 != after.size {
                        return Err(Error::Unstable(path));
                    }
                    let Ok(parsed) = sonic_rs::from_slice::<SonicValue>(&bytes) else {
                        continue;
                    };
                    let mut clean = String::new();
                    let mut raw = String::new();
                    visit_sonic_text(&parsed, &self.text_rules, &mut |_, value, format| {
                        raw.push_str(value);
                        raw.push('\u{1f}');
                        if let Some(rendered) = render_text(value, format) {
                            clean.push_str(&rendered);
                            clean.push('\u{1f}');
                        }
                    });
                    append_folded(&mut clean);
                    append_folded(&mut raw);
                    clean_main.add_file_content(&clean_skip, slot, clean.as_bytes());
                    raw_main.add_file_content(&raw_skip, slot, raw.as_bytes());
                }
                Ok(())
            },
        )?;
        source.finish()?;
        let mut clean = clean_main.compress(None);
        clean.set_skip_index(clean_skip.compress(Some(12)));
        let mut raw = raw_main.compress(None);
        raw.set_skip_index(raw_skip.compress(Some(12)));
        self.text_index = Some(TextIndex {
            clean,
            raw,
            indexed_len: self.files.len(),
        });
        Ok(())
    }

    /// Candidate file IDs in byte order. Missing regex postings scan all files.
    pub(crate) fn text_candidates(
        &self,
        pattern: Option<&str>,
        raw: bool,
        literal: bool,
    ) -> Vec<(String, Stamp)> {
        let index = self.text_index.as_ref().expect("full-text index is built");
        let bits = pattern.and_then(|pattern| {
            let postings = if raw { &index.raw } else { &index.clean };
            if literal {
                postings.query(pattern.as_bytes())
            } else {
                regex_plan::candidates(postings, pattern)
            }
        });
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                bits.as_ref().is_none_or(|bits| {
                    slot >= index.indexed_len
                        || self.changed.contains(&slot)
                        || BigramFilter::is_candidate(bits, slot)
                })
            })
            .map(|slot| {
                (
                    self.id(slot).expect("live text candidate").to_owned(),
                    self.files[slot]
                        .as_ref()
                        .expect("live text candidate")
                        .stamp
                        .clone(),
                )
            })
            .collect()
    }
}


fn append_folded(text: &mut String) {
    let folded = text.case_fold().collect::<String>();
    if folded != *text {
        text.push('\u{1e}');
        text.push_str(&folded);
    }
}
