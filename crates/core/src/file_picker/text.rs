//! Full-text postings for rendered text and individually declared fields.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

mod projection;
mod query;

pub(super) use projection::{project_malformed_text, project_text};


/// Combined visible text plus two filters for each declared full-text field.
pub(super) struct TextIndex {
    global_clean: BigramFilter,
    global_clean_present: bool,
    fields: Vec<FieldTextIndex>,
    indexed_len: usize,
    #[cfg(feature = "index-timings")]
    pub(super) rendered_compress: StageMeasurement,
    #[cfg(feature = "index-timings")]
    pub(super) field_compress: StageMeasurement,
}


/// Immutable decoded and rendered postings for one schema path.
pub(super) struct FieldTextIndex {
    pub(super) path: crate::JsonPath,
    pub(super) normal: BigramFilter,
    pub(super) clean: BigramFilter,
    pub(super) normal_present: bool,
    pub(super) clean_present: bool,
}


/// Two projections of one field in one source document.
pub(super) struct FieldTextContent {
    normal: Vec<u8>,
    clean: Vec<u8>,
}


/// Per-slot text extracted from an already parsed document.
pub(super) struct TextContent {
    pub(super) global_clean: Vec<u8>,
    pub(super) fields: Vec<FieldTextContent>,
}


/// Mutable posting builders for a field's two representations.
struct FieldTextBuilders {
    path: crate::JsonPath,
    normal_main: BigramIndexBuilder,
    normal_skip: BigramIndexBuilder,
    clean_main: BigramIndexBuilder,
    clean_skip: BigramIndexBuilder,
    normal_present: AtomicBool,
    clean_present: AtomicBool,
}


/// Mutable posting builders shared by eager and lazy full-text construction.
pub(super) struct TextIndexBuilders {
    global_clean_main: BigramIndexBuilder,
    global_clean_skip: BigramIndexBuilder,
    global_clean_present: AtomicBool,
    fields: Vec<FieldTextBuilders>,
    indexed_len: usize,
}


impl TextIndexBuilders {
    pub(super) fn new(
        indexed_len: usize,
        encoding: BigramEncoding,
        paths: &[crate::JsonPath],
    ) -> Self {
        Self {
            global_clean_main: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            global_clean_skip: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
            global_clean_present: AtomicBool::new(false),
            fields: paths
                .iter()
                .map(|path| FieldTextBuilders {
                    path: path.clone(),
                    normal_main: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
                    normal_skip: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
                    clean_main: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
                    clean_skip: BigramIndexBuilder::new_with_encoding(indexed_len, encoding),
                    normal_present: AtomicBool::new(false),
                    clean_present: AtomicBool::new(false),
                })
                .collect(),
            indexed_len,
        }
    }

    pub(super) fn add(&self, slot: usize, content: &TextContent) {
        self.add_global(slot, content);
        self.add_fields(slot, content);
    }

    #[cfg(feature = "index-timings")]
    pub(super) fn add_profiled(
        &self,
        slot: usize,
        content: &TextContent,
    ) -> (StageMeasurement, StageMeasurement) {
        let global_timer = StageTimer::start();
        self.add_global(slot, content);
        let global = global_timer.stop();
        let field_timer = StageTimer::start();
        self.add_fields(slot, content);
        (global, field_timer.stop())
    }

    fn add_global(&self, slot: usize, content: &TextContent) {
        if !content.global_clean.is_empty() {
            self.global_clean_present.store(true, Ordering::Relaxed);
        }
        self.global_clean_main.add_file_content(
            &self.global_clean_skip,
            slot,
            &content.global_clean,
        );
    }

    fn add_fields(&self, slot: usize, content: &TextContent) {
        for (builders, bytes) in self.fields.iter().zip(&content.fields) {
            if !bytes.normal.is_empty() {
                builders.normal_present.store(true, Ordering::Relaxed);
            }
            if !bytes.clean.is_empty() {
                builders.clean_present.store(true, Ordering::Relaxed);
            }
            builders
                .normal_main
                .add_file_content(&builders.normal_skip, slot, &bytes.normal);
            builders
                .clean_main
                .add_file_content(&builders.clean_skip, slot, &bytes.clean);
        }
    }

    pub(super) fn finish(self) -> TextIndex {
        #[cfg(feature = "index-timings")]
        let rendered_timer = StageTimer::start();
        let mut global_clean = self.global_clean_main.compress(None);
        global_clean.set_skip_index(self.global_clean_skip.compress(Some(12)));
        #[cfg(feature = "index-timings")]
        let rendered_compress = rendered_timer.stop();
        #[cfg(feature = "index-timings")]
        let field_timer = StageTimer::start();
        let fields = self
            .fields
            .into_iter()
            .map(|builders| {
                let mut normal = builders.normal_main.compress_field(None);
                normal.set_skip_index(builders.normal_skip.compress_field(Some(12)));
                let mut clean = builders.clean_main.compress_field(None);
                clean.set_skip_index(builders.clean_skip.compress_field(Some(12)));
                FieldTextIndex {
                    path: builders.path,
                    normal,
                    clean,
                    normal_present: builders.normal_present.load(Ordering::Relaxed),
                    clean_present: builders.clean_present.load(Ordering::Relaxed),
                }
            })
            .collect();
        #[cfg(feature = "index-timings")]
        let field_compress = field_timer.stop();
        TextIndex {
            global_clean,
            global_clean_present: self.global_clean_present.load(Ordering::Relaxed),
            fields,
            indexed_len: self.indexed_len,
            #[cfg(feature = "index-timings")]
            rendered_compress,
            #[cfg(feature = "index-timings")]
            field_compress,
        }
    }
}


impl TextIndex {
    pub(super) fn filters(&self) -> (&BigramFilter, bool, &[FieldTextIndex], usize) {
        (
            &self.global_clean,
            self.global_clean_present,
            &self.fields,
            self.indexed_len,
        )
    }

    pub(super) fn from_filters(
        global_clean: BigramFilter,
        global_clean_present: bool,
        fields: Vec<FieldTextIndex>,
        indexed_len: usize,
        encoding: BigramEncoding,
        paths: &[crate::JsonPath],
    ) -> Result<Self> {
        if global_clean.file_count() != indexed_len
            || global_clean.encoding() != encoding
            || fields.len() != paths.len()
            || fields.iter().zip(paths).any(|(field, expected)| {
                field.path != *expected
                    || field.normal.file_count() != indexed_len
                    || field.normal.encoding() != encoding
                    || field.clean.file_count() != indexed_len
                    || field.clean.encoding() != encoding
            })
        {
            return Err(Error::Cache("invalid secondary text index slots".into()));
        }
        Ok(Self {
            global_clean,
            global_clean_present,
            fields,
            indexed_len,
            #[cfg(feature = "index-timings")]
            rendered_compress: StageMeasurement::default(),
            #[cfg(feature = "index-timings")]
            field_compress: StageMeasurement::default(),
        })
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.global_clean.heap_bytes()
            + self
                .fields
                .iter()
                .map(|field| field.normal.heap_bytes() + field.clean.heap_bytes())
                .sum::<usize>()
    }
}
