use super::*;
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Schema, Transform};
use crate::repository::{Repository, RepositoryOptions, SnapshotMode};
use crate::source::{SourceFormat, TextSegment};
use crate::workload::ReturnMode;

#[test]
fn build_worker_policy_uses_detected_capacity_and_rejects_oversubscription() {
    let nz = |value| NonZeroUsize::new(value).unwrap();
    assert_eq!(resolve_build_workers(None, Some(nz(1))), Ok(1));
    assert_eq!(resolve_build_workers(None, Some(nz(8))), Ok(8));
    assert_eq!(resolve_build_workers(None, None), Ok(1));
    assert_eq!(resolve_build_workers(Some(nz(1)), None), Ok(1));
    assert_eq!(resolve_build_workers(Some(nz(2)), Some(nz(8))), Ok(2));
    assert!(resolve_build_workers(Some(nz(9)), Some(nz(8))).is_err());
    assert!(resolve_build_workers(Some(nz(2)), None).is_err());
    assert!(NonZeroUsize::new(0).is_none());
}

#[test]
fn worker_allowance_is_bounded_and_keeps_one_oversized_job_live() {
    const MIB: u64 = 1024 * 1024;
    assert_eq!(normal_batch_allowance(1), 52 * MIB);
    assert_eq!(normal_batch_allowance(2), 52 * MIB);
    assert_eq!(normal_batch_allowance(4), 64 * MIB);
    assert_eq!(normal_batch_allowance(32), 512 * MIB);
    assert_eq!(normal_batch_allowance(usize::MAX), 512 * MIB);
    assert!(fits_allowance(0, 600 * MIB, 52 * MIB));
    assert!(!fits_allowance(8 * MIB, 600 * MIB, 52 * MIB));
    assert!(fits_allowance(44 * MIB, 8 * MIB, 52 * MIB));
    assert!(!fits_allowance(45 * MIB, 8 * MIB, 52 * MIB));
}

#[test]
fn file_workers_overlap_while_admission_keeps_file_order() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};

    struct DelayedReader {
        active: AtomicUsize,
        peak: AtomicUsize,
        second_done: AtomicBool,
        fail_first: bool,
        first_failed: AtomicBool,
        parsed: AtomicUsize,
    }

    impl SourceReader for DelayedReader {
        fn name(&self) -> &'static str {
            "delayed"
        }

        fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
            Ok(vec![ByteRange::checked(
                0,
                file.len() as u64,
                file.len() as u64,
            )?])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            self.parsed.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            if bytes == b"first" {
                let started = std::time::Instant::now();
                while !self.second_done.load(Ordering::SeqCst) {
                    if started.elapsed().as_secs() > 5 {
                        return Err("second batch did not overlap first".into());
                    }
                    std::thread::yield_now();
                }
                if self.fail_first {
                    self.first_failed.store(true, Ordering::SeqCst);
                    self.active.fetch_sub(1, Ordering::SeqCst);
                    return Err("intentional parser failure".into());
                }
            } else {
                self.second_done.store(true, Ordering::SeqCst);
                if self.fail_first {
                    let started = std::time::Instant::now();
                    while !self.first_failed.load(Ordering::SeqCst) {
                        if started.elapsed().as_secs() > 5 {
                            return Err("first batch did not report failure".into());
                        }
                        std::thread::yield_now();
                    }
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-build-overlap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("0.delayed"), b"first").unwrap();
    std::fs::write(root.join("1.delayed"), b"second").unwrap();
    let mut inventory = FileInventory::new(root.clone());
    let ids = ["0.delayed", "1.delayed"].map(|name| {
        let metadata = std::fs::metadata(root.join(name)).unwrap();
        inventory
            .add(
                name.into(),
                SourceRevision::from_metadata(&metadata).unwrap(),
                None,
                CollectionId(0),
            )
            .unwrap()
    });
    let reader = std::sync::Arc::new(DelayedReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        second_done: AtomicBool::new(false),
        fail_first: false,
        first_failed: AtomicBool::new(false),
        parsed: AtomicUsize::new(0),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("delayed", 1, "delayed", reader.clone())
        .unwrap();
    let definition = CollectionDefinition {
        name: "delayed".into(),
        source_path: ".".into(),
        format: SourceFormat::registered("delayed"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema: Default::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let jobs = [&ids[..1], &ids[1..]];
    let mut admitted = Vec::new();
    let stats = process_file_batches(
        &pool,
        std::sync::Arc::new(root.clone()),
        std::sync::Arc::new(definition.clone()),
        CollectionId(0),
        sources
            .resolve(&SourceFormat::registered("delayed"))
            .unwrap(),
        &jobs,
        std::sync::Arc::new(
            inventory
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect(),
        ),
        BigramAlphabet::Ascii,
        |batch| {
            admitted.push(batch.records[0].file);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(admitted, ids);
    assert_eq!(stats.weight_limit_bytes, 52 * 1024 * 1024);
    assert_eq!(stats.peak_active_jobs, 2);
    assert!(stats.peak_ready_batches >= 1);
    assert!(stats.job_execution_ns > 0);
    assert_eq!(reader.peak.load(Ordering::SeqCst), 2);
    assert_eq!(reader.active.load(Ordering::SeqCst), 0);
    let failing = std::sync::Arc::new(DelayedReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        second_done: AtomicBool::new(false),
        fail_first: true,
        first_failed: AtomicBool::new(false),
        parsed: AtomicUsize::new(0),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("delayed", 1, "delayed", failing.clone())
        .unwrap();
    let error = process_file_batches(
        &pool,
        std::sync::Arc::new(root.clone()),
        std::sync::Arc::new(definition),
        CollectionId(0),
        sources
            .resolve(&SourceFormat::registered("delayed"))
            .unwrap(),
        &jobs,
        std::sync::Arc::new(
            inventory
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect(),
        ),
        BigramAlphabet::Ascii,
        |_| panic!("fatal batch must not be admitted"),
    )
    .unwrap_err();
    assert!(error.contains("intentional parser failure"), "{error}");
    assert_eq!(failing.parsed.load(Ordering::SeqCst), 2);
    assert_eq!(failing.active.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_pool_runs_file_batches_from_distinct_collections_together() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};

    if std::thread::available_parallelism().map_or(1, NonZeroUsize::get) < 2 {
        return;
    }

    struct CrossReader {
        active: AtomicUsize,
        peak: AtomicUsize,
        second_started: AtomicBool,
    }

    impl SourceReader for CrossReader {
        fn name(&self) -> &'static str {
            "cross_collection"
        }

        fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
            Ok(vec![ByteRange::checked(
                0,
                file.len() as u64,
                file.len() as u64,
            )?])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let started = std::time::Instant::now();
            if bytes == b"first" {
                while !self.second_started.load(Ordering::SeqCst) {
                    if started.elapsed().as_secs() > 5 {
                        return Err("second collection did not start".into());
                    }
                    std::thread::yield_now();
                }
            } else {
                self.second_started.store(true, Ordering::SeqCst);
                while self.peak.load(Ordering::SeqCst) < 2 {
                    if started.elapsed().as_secs() > 5 {
                        return Err("first collection did not overlap".into());
                    }
                    std::thread::yield_now();
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-build-cross-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for name in ["a", "b"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("a/0.cross"), b"first").unwrap();
    std::fs::write(root.join("b/0.cross"), b"second").unwrap();
    let reader = std::sync::Arc::new(CrossReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        second_started: AtomicBool::new(false),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("cross", 1, "cross", reader.clone())
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: name.into(),
            format: SourceFormat::registered("cross"),
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: Default::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect();
    let repo = Repository::open_with_sources(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            build_workers: Some(NonZeroUsize::new(2).unwrap()),
            ..Default::default()
        },
        sources,
    )
    .unwrap();
    assert_eq!(reader.peak.load(Ordering::SeqCst), 2);
    assert_eq!(reader.active.load(Ordering::SeqCst), 0);
    assert_eq!(repo.collections()[0].generation.records.len(), 1);
    assert_eq!(repo.collections()[1].generation.records.len(), 1);
    assert_eq!(repo.collections()[0].generation.files, [FileId(0)]);
    assert_eq!(repo.collections()[1].generation.files, [FileId(1)]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_large_framed_file_uses_parallel_ranges_with_identical_indexes() {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};

    struct TwoRanges {
        wait: bool,
        active: AtomicUsize,
        peak: AtomicUsize,
        second_started: AtomicBool,
    }

    impl SourceReader for TwoRanges {
        fn name(&self) -> &'static str {
            "two_ranges"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            let split = bytes.iter().position(|byte| *byte == b'\n').unwrap();
            let size = bytes.len() as u64;
            Ok(vec![
                ByteRange::checked(0, split as u64, size)?,
                ByteRange::checked(split as u64 + 1, size, size)?,
            ])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let title = match bytes[0] {
                b'A' => {
                    if self.wait {
                        let started = std::time::Instant::now();
                        while !self.second_started.load(Ordering::SeqCst) {
                            if started.elapsed().as_secs() > 5 {
                                return Err("second range was not scheduled".into());
                            }
                            std::thread::yield_now();
                        }
                    }
                    "Alpha"
                }
                b'B' => {
                    self.second_started.store(true, Ordering::SeqCst);
                    if self.wait {
                        let started = std::time::Instant::now();
                        while self.peak.load(Ordering::SeqCst) < 2 {
                            if started.elapsed().as_secs() > 5 {
                                return Err("first range was not scheduled".into());
                            }
                            std::thread::yield_now();
                        }
                    }
                    "Beta"
                }
                _ => return Err("unexpected record".into()),
            };
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(BTreeMap::from([(
                    "title".into(),
                    Value::String(title.into()),
                )])),
                text: vec![TextSegment {
                    path: vec!["title".into()],
                    text: title.into(),
                }],
            })
        }
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-build-framed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source_bytes = vec![b'a'; 4 * 1024 * 1024 + 1];
    source_bytes[0] = b'A';
    source_bytes.push(b'\n');
    source_bytes.extend(std::iter::repeat_n(b'b', 4 * 1024 * 1024 + 1));
    source_bytes[4 * 1024 * 1024 + 2] = b'B';
    std::fs::write(root.join("data/large.two"), &source_bytes).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::registered("two"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("title").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let build = |workers, wait| {
        let reader = std::sync::Arc::new(TwoRanges {
            wait,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            second_started: AtomicBool::new(false),
        });
        let mut sources = SourceRegistry::builtin();
        sources.register("two", 1, "two", reader.clone()).unwrap();
        let repo = Repository::open_with_sources(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: Some(NonZeroUsize::new(workers).unwrap()),
                ..Default::default()
            },
            sources,
        )
        .unwrap();
        (repo, reader)
    };
    let (one, serial_reader) = build(1, false);
    let (parallel, parallel_reader) = build(parallelism.min(2), parallelism >= 2);
    assert_eq!(one.collections()[0].generation.records.len(), 2);
    assert_eq!(one.build_parses(), 2);
    assert_eq!(parallel.build_parses(), 2);
    assert_eq!(serial_reader.peak.load(Ordering::SeqCst), 1);
    if parallelism >= 2 {
        assert_eq!(parallel_reader.peak.load(Ordering::SeqCst), 2);
    }
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&parallel.collections()[0].generation).unwrap()
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        parallel.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].raw.skip,
        parallel.collections()[0].raw.skip
    );
    let path = &vec!["title".to_owned()];
    let one_posting = &one.collections()[0].postings.fields[path];
    let parallel_posting = &parallel.collections()[0].postings.fields[path];
    assert_eq!(one_posting.values, parallel_posting.values);
    assert_eq!(one_posting.presence, parallel_posting.presence);
    let sort_path = FieldPath::parse("title").unwrap();
    let one_sort = &one.collections()[0].sorting.columns[&sort_path];
    let parallel_sort = &parallel.collections()[0].sorting.columns[&sort_path];
    assert_eq!(one_sort.keys, parallel_sort.keys);
    assert_eq!(one_sort.ascending, parallel_sort.ascending);
    assert_eq!(one_sort.descending, parallel_sort.descending);
    assert_eq!(one.collections()[0].generation.files.len(), 1);
    assert_eq!(
        one.collections()[0]
            .generation
            .file_records
            .records_for(FileId(0))
            .unwrap()
            .len(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_jsonl_preserves_malformed_siblings_and_fallback_grams() {
    let root = std::env::temp_dir().join(format!(
        "objekte-build-oversized-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut bytes = b"{\"title\":\"".to_vec();
    bytes.extend(std::iter::repeat_n(b'a', 8 * 1024 * 1024));
    bytes.extend_from_slice(b"\"}\n{\"title\":\"healthy\"}\n{\"title\":\n{\"title\":\"");
    bytes.push(0xff);
    bytes.extend_from_slice(b"\"}\n");
    std::fs::write(root.join("data/large.jsonl"), bytes).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("@file.status").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let build = |workers| {
        Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: Some(NonZeroUsize::new(workers).unwrap()),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let one = build(1);
    let parallel = build(available.min(2));
    assert_eq!(one.build_parses(), 2);
    assert_eq!(one.build_failed_parses(), 2);
    for repo in [&one, &parallel] {
        assert_eq!(repo.collections()[0].generation.records.len(), 4);
        assert_eq!(repo.files().files[0].status, FileStatus::Broken);
        assert_eq!(repo.files().files[0].diagnostics.len(), 3);
        assert_eq!(
            repo.collections()[0]
                .generation
                .file_records
                .records_for(FileId(0))
                .unwrap()
                .len(),
            4
        );
        assert_eq!(repo.collections()[0].logical_records.len(), 2);
        for record in repo.collections()[0].generation.records.iter().skip(2) {
            assert_eq!(record.class, RecordClass::RawFallback);
            assert_eq!(
                repo.files()
                    .diagnostic(record.file, record.diagnostic_ref)
                    .unwrap()
                    .record,
                Some(record.slot)
            );
        }
    }
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&parallel.collections()[0].generation).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&one.files().files[0].diagnostics).unwrap(),
        serde_json::to_value(&parallel.files().files[0].diagnostics).unwrap()
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        parallel.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].raw.skip,
        parallel.collections()[0].raw.skip
    );
    assert_eq!(
        one.collections()[0].fallback_text.normal,
        parallel.collections()[0].fallback_text.normal
    );
    assert_eq!(
        one.collections()[0].fallback_text.skip,
        parallel.collections()[0].fallback_text.skip
    );
    for filter in ["title = 'healthy'", "@record.class = 1"] {
        let query = Query {
            collections: vec!["data".into()],
            filter: Some(parse_filter(filter).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let a = one.search(&query).unwrap();
        let b = parallel.search(&query).unwrap();
        assert_eq!((a.count, a.hits), (b.count, b.hits), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_batches_bound_count_and_source_bytes_without_losing_empty_files() {
    let mut inventory = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for index in 0..600 {
        ids.push(
            inventory
                .add(
                    format!("{index:04}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: 0,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &inventory)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [512, 88]
    );
    let mut large = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for (index, size) in [5, 5, 9, 1].into_iter().enumerate() {
        ids.push(
            large
                .add(
                    format!("{index}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: size * 1024 * 1024,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &large)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [1, 1, 1, 1]
    );
}

#[test]
fn bounded_batch_admission_keeps_slots_diagnostics_and_complete_gram_maps() {
    let root = std::env::temp_dir().join(format!("objekte-build-batch-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..512 {
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            b"{\"title\":\"x\"}",
        )
        .unwrap();
    }
    std::fs::write(root.join("data/0512.jsonl"), b"{\"title\":").unwrap();
    std::fs::write(root.join("data/0513.jsonl"), b"{\"title\":\"y\"}").unwrap();
    std::fs::write(root.join("data/0514.jsonl"), b"").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("@file.status").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            build_workers: Some(NonZeroUsize::new(1).unwrap()),
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        let parallel = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Rebuild,
                build_workers: NonZeroUsize::new(
                    std::thread::available_parallelism()
                        .map_or(1, NonZeroUsize::get)
                        .min(4),
                ),
                ..options
            },
        )
        .unwrap();
        assert!(!parallel.startup().restored);
        for repo in [&built, &restored, &parallel] {
            let collection = &repo.collections()[0];
            assert_eq!(collection.generation.files.len(), 515);
            assert_eq!(collection.generation.records.len(), 514);
            assert_eq!(
                collection.generation.file_records.records_for(FileId(514)),
                Some([].as_slice())
            );
            assert_eq!(repo.files().files[512].status, FileStatus::Broken);
            let malformed = &collection.generation.records[512];
            assert_eq!(malformed.class, RecordClass::RawFallback);
            assert_eq!(
                repo.files()
                    .diagnostic(malformed.file, malformed.diagnostic_ref)
                    .unwrap()
                    .record,
                Some(malformed.slot)
            );
            for (filter, count, first_id) in [
                ("title = 'x'", 512, "0000"),
                ("title = 'y'", 1, "0513"),
                ("@record.class = 1", 1, "~fallback:"),
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    filter: Some(parse_filter(filter).unwrap()),
                    fts: None,
                    options: QueryOptions {
                        sort: vec![],
                        limit: 3,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let result = repo.search(&query).unwrap();
                assert_eq!(result.count, Some(count), "{filter}");
                assert!(result.hits[0].record_id.starts_with(first_id), "{filter}");
                assert_eq!(result.counters.files_opened, 0, "{filter}");
                assert_eq!(
                    result.hits,
                    repo.search_reference(&query).unwrap().hits,
                    "{filter}"
                );
            }
            let mut direct = BigramIndex::new(alphabet);
            for slot in 0..514 {
                let text = match slot {
                    0..=511 => Some("x"),
                    513 => Some("y"),
                    _ => None,
                };
                if let Some(text) = text {
                    direct
                        .add_record(
                            RecordSlot(slot),
                            &[TextSegment {
                                path: vec!["title".into()],
                                text: text.into(),
                            }],
                        )
                        .unwrap();
                } else {
                    direct.add_empty(RecordSlot(slot)).unwrap();
                }
            }
            assert_eq!(collection.raw.normal, direct.normal);
            assert_eq!(collection.raw.skip, direct.skip);
            assert_eq!(collection.custom_text.len(), 1);
            let custom = collection.custom_text.values().next().unwrap();
            assert_eq!(custom.normal, direct.normal);
            assert_eq!(custom.skip, direct.skip);
        }
        let a = &built.collections()[0];
        let b = &parallel.collections()[0];
        assert_eq!(
            serde_json::to_value(&a.generation).unwrap(),
            serde_json::to_value(&b.generation).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&built.files().files).unwrap(),
            serde_json::to_value(&parallel.files().files).unwrap()
        );
        assert_eq!(a.logical_records, b.logical_records);
        assert_eq!(a.raw.normal, b.raw.normal);
        assert_eq!(a.raw.skip, b.raw.skip);
        assert_eq!(a.fallback_text.normal, b.fallback_text.normal);
        assert_eq!(a.fallback_text.skip, b.fallback_text.skip);
        for (path, posting) in &a.postings.fields {
            let other = &b.postings.fields[path];
            assert_eq!(posting.covered_records, other.covered_records);
            assert_eq!(posting.presence, other.presence);
            assert_eq!(posting.nulls, other.nulls);
            assert_eq!(posting.values, other.values);
            assert_eq!(posting.lengths, other.lengths);
        }
        for (path, column) in &a.sorting.columns {
            let other = &b.sorting.columns[path];
            assert_eq!(column.keys, other.keys);
            assert_eq!(column.ascending, other.ascending);
            assert_eq!(column.descending, other.descending);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_file_batches_reconcile_diagnostic_caps_and_healthy_siblings() {
    let root = std::env::temp_dir().join(format!("objekte-build-shared-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut contents = "{\"title\":\n".repeat(70);
    contents.push_str("{\"title\":\"ok\"}\n");
    std::fs::write(root.join("data/mixed.jsonl"), contents).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![FieldPath::parse("@file.status").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect::<Vec<_>>();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
    for repo in [&built, &restored] {
        let file = &repo.files().files[0];
        assert_eq!(file.status, FileStatus::Broken);
        assert_eq!(file.diagnostics.len(), 64);
        assert_eq!(file.omitted_diagnostics, 76);
        for collection in repo.collections() {
            assert_eq!(collection.generation.records.len(), 71);
            assert_eq!(
                collection
                    .generation
                    .file_records
                    .records_for(FileId(0))
                    .unwrap()
                    .len(),
                71
            );
            assert_eq!(
                collection.generation.records[70].class,
                RecordClass::Logical
            );
        }
        let query = |filter| Query {
            collections: vec![],
            filter: Some(parse_filter(filter).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 3,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        for (filter, count) in [
            ("@record.class = 1", 140),
            ("title = 'ok'", 2),
            ("@file.status = 1", 142),
        ] {
            let result = repo.search(&query(filter)).unwrap();
            assert_eq!(result.count, Some(count), "{filter}");
            assert_eq!(result.counters.files_opened, 0, "{filter}");
            assert_eq!(
                result.hits,
                repo.search_reference(&query(filter)).unwrap().hits
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_intrinsic_ids_across_batches_fail_before_publication() {
    let root = std::env::temp_dir().join(format!("objekte-build-duplicate-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..513 {
        let id = if number == 0 || number == 512 {
            "same".into()
        } else {
            format!("id-{number}")
        };
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            format!("{{\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let error = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .err()
    .unwrap();
    assert!(
        error.contains("duplicate record ID") && error.contains("0512.jsonl"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
