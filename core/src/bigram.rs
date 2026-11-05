//! Conservative record-level bigram candidates over separate text segments.

use std::collections::{HashMap, HashSet};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::model::RecordSlot;
use crate::source::TextSegment;
use crate::workload::BigramAlphabet;

pub const BIGRAM_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BigramIndex {
    pub alphabet: BigramAlphabet,
    pub record_count: u32,
    pub normal: HashMap<u64, RoaringBitmap>,
    pub skip: HashMap<u64, RoaringBitmap>,
}

impl BigramIndex {
    pub fn new_ascii() -> Self {
        Self::new(BigramAlphabet::Ascii)
    }

    pub fn new(alphabet: BigramAlphabet) -> Self {
        Self {
            alphabet,
            record_count: 0,
            normal: HashMap::new(),
            skip: HashMap::new(),
        }
    }

    pub fn add_record(&mut self, slot: RecordSlot, segments: &[TextSegment]) -> Result<(), String> {
        if slot.0 != self.record_count {
            return Err("bigram slots must be added in record order".into());
        }
        self.record_count = self
            .record_count
            .checked_add(1)
            .ok_or("bigram record overflow")?;
        let mut normal = HashSet::new();
        let mut skip = HashSet::new();
        for segment in segments {
            match self.alphabet {
                BigramAlphabet::Ascii => {
                    let bytes = segment.text.as_bytes();
                    for pair in bytes.windows(2) {
                        if let Some(key) = ascii_pair(pair[0], pair[1]) {
                            normal.insert(key);
                        }
                    }
                    for triple in bytes.windows(3) {
                        if let Some(key) = ascii_pair(triple[0], triple[2]) {
                            skip.insert(key);
                        }
                    }
                }
                BigramAlphabet::Unicode => {
                    let chars: Vec<char> = segment.text.chars().collect();
                    for pair in chars.windows(2) {
                        normal.insert(unicode_pair(pair[0], pair[1]));
                    }
                    for triple in chars.windows(3) {
                        skip.insert(unicode_pair(triple[0], triple[2]));
                    }
                }
            }
        }
        for gram in normal {
            self.normal.entry(gram).or_default().insert(slot.0);
        }
        for gram in skip {
            self.skip.entry(gram).or_default().insert(slot.0);
        }
        Ok(())
    }

    /// `None` means no safe useful restriction; an empty bitmap is a proven
    /// absence of this necessary gram over complete index coverage.
    pub fn candidates(&self, literal: &str) -> Option<RoaringBitmap> {
        let mut grams = HashSet::new();
        match self.alphabet {
            BigramAlphabet::Ascii => {
                let bytes = literal.as_bytes();
                if bytes.len() < 2 || !bytes.iter().all(|b| (32..=126).contains(b)) {
                    return None;
                }
                for pair in bytes.windows(2) {
                    grams.insert((false, ascii_pair(pair[0], pair[1]).unwrap()));
                }
                for triple in bytes.windows(3) {
                    grams.insert((true, ascii_pair(triple[0], triple[2]).unwrap()));
                }
            }
            BigramAlphabet::Unicode => {
                let chars: Vec<char> = literal.chars().collect();
                if chars.len() < 2 {
                    return None;
                }
                for pair in chars.windows(2) {
                    grams.insert((false, unicode_pair(pair[0], pair[1])));
                }
                for triple in chars.windows(3) {
                    grams.insert((true, unicode_pair(triple[0], triple[2])));
                }
            }
        }
        let mut columns = Vec::new();
        for (skip, key) in grams {
            let column = if skip {
                self.skip.get(&key)
            } else {
                self.normal.get(&key)
            };
            let Some(column) = column else {
                return Some(RoaringBitmap::new());
            };
            columns.push(column);
        }
        columns.sort_by_key(|column| column.len());
        let mut result = (*columns.first()?).clone();
        for column in columns.into_iter().skip(1) {
            result &= column;
            if result.is_empty() {
                break;
            }
        }
        Some(result)
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        if self.record_count as usize != record_count {
            return Err("bigram coverage mismatch".into());
        }
        if self
            .normal
            .values()
            .chain(self.skip.values())
            .any(|column| column.max().is_some_and(|slot| slot >= self.record_count))
        {
            return Err("bigram slot out of bounds".into());
        }
        Ok(())
    }

    pub fn estimated_heap_bytes(&self) -> usize {
        self.normal
            .values()
            .chain(self.skip.values())
            .map(|set| set.serialized_size() as usize + 48)
            .sum()
    }
}

fn ascii_pair(a: u8, b: u8) -> Option<u64> {
    if !(32..=126).contains(&a) || !(32..=126).contains(&b) {
        return None;
    }
    Some(((a.to_ascii_lowercase() as u64) << 8) | b.to_ascii_lowercase() as u64)
}

fn unicode_pair(a: char, b: char) -> u64 {
    ((a as u64) << 21) | b as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_ascii_candidates_keep_segments_and_records_separate() {
        let mut index = BigramIndex::new_ascii();
        for (slot, parts) in [
            vec!["hello", "world"],
            vec!["hel", "loworld"],
            vec!["xhelloworldy"],
        ]
        .into_iter()
        .enumerate()
        {
            let segments = parts
                .into_iter()
                .map(|s| TextSegment {
                    path: vec!["text".into()],
                    text: s.into(),
                })
                .collect::<Vec<_>>();
            index
                .add_record(RecordSlot(slot as u32), &segments)
                .unwrap();
        }
        index.validate(3).unwrap();
        let candidates = index.candidates("helloworld").unwrap();
        assert!(candidates.contains(2));
        assert!(!candidates.contains(0));
        assert!(index.candidates("ü").is_none());
        assert!(index.candidates("a").is_none());
        assert!(index.candidates("unseen bigram").unwrap().is_empty());
    }

    #[test]
    fn ascii_and_unicode_candidates_are_conservative() {
        let segments = ["Müller", "Straße", "Leipzig"].map(|text| {
            vec![TextSegment {
                path: vec!["name".into()],
                text: text.into(),
            }]
        });
        let mut ascii = BigramIndex::new(BigramAlphabet::Ascii);
        let mut unicode = BigramIndex::new(BigramAlphabet::Unicode);
        for (slot, record) in segments.iter().enumerate() {
            ascii.add_record(RecordSlot(slot as u32), record).unwrap();
            unicode.add_record(RecordSlot(slot as u32), record).unwrap();
        }
        for literal in ["Müller", "Straße", "Leipzig", "ü", "not there"] {
            for (slot, record) in segments.iter().enumerate() {
                if record[0].text.contains(literal) {
                    assert!(
                        ascii
                            .candidates(literal)
                            .is_none_or(|set| set.contains(slot as u32))
                    );
                    assert!(
                        unicode
                            .candidates(literal)
                            .is_none_or(|set| set.contains(slot as u32))
                    );
                }
            }
        }
        assert_eq!(ascii.alphabet, BigramAlphabet::Ascii);
        assert_eq!(unicode.alphabet, BigramAlphabet::Unicode);
    }
}
