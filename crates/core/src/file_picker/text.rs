//! Full-text postings for rendered text and individually declared fields.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

mod projection;
mod query;

pub(super) use projection::{
    project_malformed_text, project_malformed_text_into, project_text, project_text_into,
};

/// Rendered text outside declared fields plus two filters for each field.
pub(super) struct TextIndex {
    global_clean: BigramFilter,
    global_clean_present: bool,
    pub(super) global_clean_observed: Vec<u64>,
    pub(super) fields: Vec<FieldTextIndex>,
    indexed_len: usize,
    #[cfg(feature = "index-timings")]
    pub(super) rendered_compress: StageMeasurement,
    #[cfg(feature = "index-timings")]
    pub(super) field_compress: StageMeasurement,
    #[cfg(feature = "index-timings")]
    pub(super) text_compress_nanos: u64,
}

/// Immutable decoded and rendered postings for one schema path.
pub(super) struct FieldTextIndex {
    pub(super) path: crate::JsonPath,
    pub(super) normal: BigramFilter,
    pub(super) clean: BigramFilter,
    pub(super) normal_present: bool,
    pub(super) clean_present: bool,
    pub(super) normal_observed: Vec<u64>,
    pub(super) clean_observed: Vec<u64>,
}

/// Two projections of one field in one source document.
pub(super) struct FieldTextContent {
    normal: String,
    clean: String,
}

/// Per-slot text extracted from an already parsed document.
pub(super) struct TextContent {
    pub(super) global_clean: String,
    pub(super) fields: Vec<FieldTextContent>,
}

impl TextContent {
    pub(super) fn new(field_count: usize) -> Self {
        Self {
            global_clean: String::new(),
            fields: (0..field_count)
                .map(|_| FieldTextContent {
                    normal: String::new(),
                    clean: String::new(),
                })
                .collect(),
        }
    }

    fn clear(&mut self) {
        self.global_clean.clear();
        for field in &mut self.fields {
            field.normal.clear();
            field.clean.clear();
        }
    }
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

/// One independently owned consecutive/skip-one posting pair.
struct CompressionJob {
    order: usize,
    main: BigramIndexBuilder,
    skip: BigramIndexBuilder,
    field: bool,
}

struct CompressedJob {
    order: usize,
    filter: BigramFilter,
    observed: Vec<u64>,
    #[cfg(feature = "index-timings")]
    duration: StageMeasurement,
}

fn compress_pair(job: CompressionJob) -> CompressedJob {
    #[cfg(feature = "index-timings")]
    let timer = StageTimer::start();
    let observed = job.main.observed_keys();
    let mut filter = if job.field {
        job.main.compress_field(None)
    } else {
        job.main.compress(None)
    };
    filter.set_skip_index(if job.field {
        job.skip.compress_field(Some(12))
    } else {
        job.skip.compress(Some(12))
    });
    CompressedJob {
        order: job.order,
        filter,
        observed,
        #[cfg(feature = "index-timings")]
        duration: timer.stop(),
    }
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
            content.global_clean.as_bytes(),
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
            builders.normal_main.add_file_content(
                &builders.normal_skip,
                slot,
                bytes.normal.as_bytes(),
            );
            builders.clean_main.add_file_content(
                &builders.clean_skip,
                slot,
                bytes.clean.as_bytes(),
            );
        }
    }

    pub(super) fn finish(self) -> TextIndex {
        #[cfg(feature = "index-timings")]
        let phase_timer = StageTimer::start();
        let mut jobs = vec![CompressionJob {
            order: 0,
            main: self.global_clean_main,
            skip: self.global_clean_skip,
            field: false,
        }];
        let mut metadata = Vec::with_capacity(self.fields.len());
        for (index, builders) in self.fields.into_iter().enumerate() {
            metadata.push((
                builders.path,
                builders.normal_present.load(Ordering::Relaxed),
                builders.clean_present.load(Ordering::Relaxed),
            ));
            jobs.push(CompressionJob {
                order: 1 + index * 2,
                main: builders.normal_main,
                skip: builders.normal_skip,
                field: true,
            });
            jobs.push(CompressionJob {
                order: 2 + index * 2,
                main: builders.clean_main,
                skip: builders.clean_skip,
                field: true,
            });
        }
        // Two tasks can use the shared Rayon pool inside sparse compression.
        // Assign alternating filters to each task without copying their slabs.
        let mut first = Vec::new();
        let mut second = Vec::new();
        for (index, job) in jobs.into_iter().enumerate() {
            if index % 2 == 0 {
                first.push(job);
            } else {
                second.push(job);
            }
        }
        let compress_jobs =
            |jobs: Vec<CompressionJob>| jobs.into_iter().map(compress_pair).collect::<Vec<_>>();
        let (first, second) = if self.indexed_len >= 8192 && !second.is_empty() {
            rayon::join(|| compress_jobs(first), || compress_jobs(second))
        } else {
            (compress_jobs(first), compress_jobs(second))
        };
        let mut compressed = first.into_iter().chain(second).collect::<Vec<_>>();
        compressed.sort_unstable_by_key(|job| job.order);
        #[cfg(feature = "index-timings")]
        let text_compress_nanos = phase_timer.stop().wall_nanos;
        #[cfg(feature = "index-timings")]
        let rendered_compress = compressed[0].duration;
        #[cfg(feature = "index-timings")]
        let field_compress =
            compressed
                .iter()
                .skip(1)
                .fold(StageMeasurement::default(), |mut total, job| {
                    total += job.duration;
                    total
                });
        let mut compressed = compressed.into_iter();
        let global = compressed.next().expect("global rendered index");
        let global_clean = global.filter;
        let global_clean_observed = global.observed;
        let fields = metadata
            .into_iter()
            .map(|(path, normal_present, clean_present)| {
                let normal = compressed.next().expect("normal field index");
                let clean = compressed.next().expect("rendered field index");
                FieldTextIndex {
                    path,
                    normal: normal.filter,
                    clean: clean.filter,
                    normal_present,
                    clean_present,
                    normal_observed: normal.observed,
                    clean_observed: clean.observed,
                }
            })
            .collect();
        TextIndex {
            global_clean,
            global_clean_present: self.global_clean_present.load(Ordering::Relaxed),
            global_clean_observed,
            fields,
            indexed_len: self.indexed_len,
            #[cfg(feature = "index-timings")]
            rendered_compress,
            #[cfg(feature = "index-timings")]
            field_compress,
            #[cfg(feature = "index-timings")]
            text_compress_nanos,
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
        global_clean_observed: Vec<u64>,
        fields: Vec<FieldTextIndex>,
        indexed_len: usize,
        encoding: BigramEncoding,
        paths: &[crate::JsonPath],
    ) -> Result<Self> {
        if global_clean.file_count() != indexed_len
            || global_clean.encoding() != encoding
            || global_clean_observed.len() != encoding.key_slots().div_ceil(64)
            || fields.len() != paths.len()
            || fields.iter().zip(paths).any(|(field, expected)| {
                field.path != *expected
                    || field.normal.file_count() != indexed_len
                    || field.normal.encoding() != encoding
                    || field.clean.file_count() != indexed_len
                    || field.clean.encoding() != encoding
                    || field.normal_observed.len() != encoding.key_slots().div_ceil(64)
                    || field.clean_observed.len() != encoding.key_slots().div_ceil(64)
            })
        {
            return Err(Error::Cache("invalid secondary text index slots".into()));
        }
        Ok(Self {
            global_clean,
            global_clean_present,
            global_clean_observed,
            fields,
            indexed_len,
            #[cfg(feature = "index-timings")]
            rendered_compress: StageMeasurement::default(),
            #[cfg(feature = "index-timings")]
            field_compress: StageMeasurement::default(),
            #[cfg(feature = "index-timings")]
            text_compress_nanos: 0,
        })
    }

    pub(super) fn heap_bytes(&self) -> usize {
        self.global_clean.heap_bytes()
            + self.global_clean_observed.len() * 8
            + self
                .fields
                .iter()
                .map(|field| {
                    field.normal.heap_bytes()
                        + field.clean.heap_bytes()
                        + (field.normal_observed.len() + field.clean_observed.len()) * 8
                })
                .sum::<usize>()
    }
}
