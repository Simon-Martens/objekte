//! Bigram postings for every included JSON leaf value.

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


/// Per-slot rendered and raw text extracted from an already parsed document.
pub(super) struct TextContent {
    pub(super) clean: Vec<u8>,
    pub(super) raw: Vec<u8>,
}


/// Mutable posting builders shared by eager and lazy full-text construction.
pub(super) struct TextIndexBuilders {
    clean_main: BigramIndexBuilder,
    clean_skip: BigramIndexBuilder,
    raw_main: BigramIndexBuilder,
    raw_skip: BigramIndexBuilder,
    indexed_len: usize,
}


impl TextIndexBuilders {
    pub(super) fn new(indexed_len: usize, encoding: BigramEncoding) -> Self {
        Self {
            clean_main: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            clean_skip: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            raw_main: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            raw_skip: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            indexed_len,
        }
    }

    pub(super) fn add(&self, slot: usize, content: &TextContent) {
        self.clean_main
            .add_file_content(&self.clean_skip, slot, &content.clean);
        self.raw_main
            .add_file_content(&self.raw_skip, slot, &content.raw);
    }

    pub(super) fn finish(self) -> TextIndex {
        let mut clean = self.clean_main.compress(None);
        clean.set_skip_index(self.clean_skip.compress(Some(12)));
        let mut raw = self.raw_main.compress(None);
        raw.set_skip_index(self.raw_skip.compress(Some(12)));
        TextIndex {
            clean,
            raw,
            indexed_len: self.indexed_len,
        }
    }
}


pub(super) fn project_text(value: &SonicValue, rules: &TextRules) -> TextContent {
    let mut clean = String::new();
    let mut raw = String::new();
    visit_sonic_text(value, rules, &mut |_, value, format| {
        raw.push_str(value);
        raw.push('\u{1f}');
        if let Some(rendered) = render_text(value, format) {
            clean.push_str(&rendered);
            clean.push('\u{1f}');
        }
    });
    append_folded(&mut clean);
    append_folded(&mut raw);
    TextContent {
        clean: clean.into_bytes(),
        raw: raw.into_bytes(),
    }
}

/// Malformed files have no trustworthy field paths or markup declarations.
pub(super) fn project_malformed_text(text: &str) -> TextContent {
    let mut clean = render_text(text, crate::TextFormat::Plain).expect("plain text renders");
    let mut raw = text.to_owned();
    append_folded(&mut clean);
    append_folded(&mut raw);
    TextContent {
        clean: clean.into_bytes(),
        raw: raw.into_bytes(),
    }
}


impl TextIndex {
    pub(super) fn save_parts(&self) -> Result<(Vec<u8>, Vec<u8>, usize)> {
        Ok((
            self.clean.to_bytes()?,
            self.raw.to_bytes()?,
            self.indexed_len,
        ))
    }

    pub(super) fn restore_parts(
        clean: &[u8],
        raw: &[u8],
        indexed_len: usize,
        encoding: BigramEncoding,
    ) -> Result<Self> {
        let clean = BigramFilter::from_bytes(clean)?;
        let raw = BigramFilter::from_bytes(raw)?;
        if clean.file_count() != indexed_len
            || raw.file_count() != indexed_len
            || clean.encoding() != encoding
            || raw.encoding() != encoding
        {
            return Err(Error::Cache("invalid secondary text index slots".into()));
        }
        Ok(Self {
            clean,
            raw,
            indexed_len,
        })
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.clean.heap_bytes() + self.raw.heap_bytes()
    }
}


impl FilePicker {
    pub(crate) fn text_index_bytes(&self) -> Option<usize> {
        self.text_index.as_ref().map(TextIndex::heap_bytes)
    }

    /// Ensure the separate postings required by markup or exclusion rules.
    pub(crate) fn ensure_text_index(&mut self) -> Result<bool> {
        if !self.eager_text_index {
            return Ok(false);
        }
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
        let builders = TextIndexBuilders::new(self.files.len(), self.encoding);
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
                    let content = if self.malformed.contains(&slot) {
                        project_malformed_text(&crate::text::malformed_search_text(&bytes))
                    } else {
                        match sonic_rs::from_slice::<SonicValue>(&bytes) {
                            Ok(parsed) => project_text(&parsed, &self.text_rules),
                            Err(_) => {
                                project_malformed_text(&crate::text::malformed_search_text(&bytes))
                            }
                        }
                    };
                    builders.add(slot, &content);
                }
                Ok(())
            },
        )?;
        source.finish()?;
        self.text_index = Some(builders.finish());
        Ok(())
    }

    /// Candidate file IDs in ID or modification-time order. Missing regex postings scan all files.
    pub(crate) fn text_candidates(
        &self,
        pattern: Option<&str>,
        raw: bool,
        literal: bool,
        newest_first: bool,
    ) -> Vec<(String, Stamp)> {
        let postings = self.text_index.as_ref().map_or(&self.index, |index| {
            if raw { &index.raw } else { &index.clean }
        });
        let indexed_len = self
            .text_index
            .as_ref()
            .map_or(self.indexed_len, |index| index.indexed_len);
        let bits = pattern.and_then(|pattern| {
            if literal {
                postings.query(pattern.as_bytes())
            } else {
                regex_plan::candidates(postings, pattern)
            }
        });
        let slots: Box<dyn Iterator<Item = usize> + '_> = if newest_first {
            Box::new(
                self.modified_order
                    .iter(&self.files, &self.id_bytes)
                    .map(|slot| slot as usize),
            )
        } else {
            Box::new(self.sorted.iter().copied())
        };
        slots
            .filter(|&slot| {
                bits.as_ref().is_none_or(|bits| {
                    slot >= indexed_len
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
