use super::*;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::inference::{InferenceConfig, InferenceMode};
use crate::inventory::ByteRange;
use crate::model::{FileId, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::FieldDefinition;
use crate::source::{ExtractedRecord, JsonLinesReader, SourceFormat, SourceReader, SourceRegistry};
use crate::workload::ReturnMode;

struct CountingJsonLinesReader {
    parses: Arc<AtomicUsize>,
}

impl SourceReader for CountingJsonLinesReader {
    fn name(&self) -> &'static str {
        "counting-jsonl"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        JsonLinesReader.frame(file)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        JsonLinesReader.parse(bytes)
    }
}

#[test]
fn start_build_is_the_single_scan_and_repeated_waits_do_not_rebuild() {
    let root = std::env::temp_dir().join(format!(
        "objekte-single-build-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, rank) in [("a", 10), ("b", 20), ("c", 20)] {
        std::fs::write(
            root.join(format!("data/{name}.counted-jsonl")),
            format!("{{\"id\":\"{name}\",\"rank\":{rank}}}\n"),
        )
        .unwrap();
    }
    let parses = Arc::new(AtomicUsize::new(0));
    let mut sources = SourceRegistry::builtin();
    sources
        .register(
            "counted-jsonl",
            1,
            "counted-jsonl",
            Arc::new(CountingJsonLinesReader {
                parses: Arc::clone(&parses),
            }),
        )
        .unwrap();
    let mut definition =
        CollectionDefinition::folder("data", "data", SourceFormat::registered("counted-jsonl"));
    definition.identity = crate::repository::IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["rank".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let task = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        sources,
    )
    .unwrap();
    let first = task.wait_complete().unwrap();
    let second = task.wait_complete().unwrap();
    assert_eq!(parses.load(Ordering::Relaxed), 3);
    assert_eq!(first.view().unwrap().startup_stats().build_source_reads, 3);
    assert_eq!(second.view().unwrap().startup_stats().build_source_reads, 3);
    let query = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(
        first
            .view()
            .unwrap()
            .count_exact(&query("rank = 20"))
            .unwrap(),
        2
    );
    drop(task);
    drop(first);
    let parses_before_empty = parses.load(Ordering::Relaxed);
    let second_view = second.view().unwrap();
    let repository = second_view.repository();
    let empty = repository.empty_like().unwrap();
    assert_eq!(parses.load(Ordering::Relaxed), parses_before_empty);
    assert_eq!(empty.collections()[0].generation.records.len(), 0);
    assert_eq!(empty.collections()[0].generation.files.len(), 0);
    assert!(
        empty.collections()[0]
            .postings
            .fields
            .values()
            .all(|posting| posting.presence.is_empty() && posting.values.is_empty())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn start_build_rejects_invalid_segment_limits_before_dispatch() {
    if usize::BITS < 64 {
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "objekte-invalid-segment-policy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.counted-jsonl"), b"{\"id\":\"a\"}\n").unwrap();
    let parses = Arc::new(AtomicUsize::new(0));
    let mut sources = SourceRegistry::builtin();
    sources
        .register(
            "counted-jsonl",
            1,
            "counted-jsonl",
            Arc::new(CountingJsonLinesReader {
                parses: Arc::clone(&parses),
            }),
        )
        .unwrap();
    let definition =
        CollectionDefinition::folder("data", "data", SourceFormat::registered("counted-jsonl"));
    let oversized = NonZeroUsize::new(u32::MAX as usize + 1).unwrap();
    for (policy, expected) in [
        (
            crate::native_segment::SegmentPolicy {
                target_records: oversized,
                max_files: NonZeroUsize::new(1).unwrap(),
            },
            "segment target exceeds the supported local-slot range",
        ),
        (
            crate::native_segment::SegmentPolicy {
                target_records: NonZeroUsize::new(1).unwrap(),
                max_files: oversized,
            },
            "segment file limit exceeds the supported local-slot range",
        ),
    ] {
        let result = Index::start_build(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                segment_policy: policy,
                ..Default::default()
            },
            sources.clone(),
        );
        assert!(matches!(
            result,
            Err(IndexError::Build(message))
                if message == expected
        ));
    }
    assert_eq!(parses.load(Ordering::Relaxed), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_conversion_rejects_retained_owner_and_view_promptly() {
    let root = std::env::temp_dir().join(format!(
        "objekte-repository-conversion-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let build = || {
        let repository = Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .unwrap();
        Index::from_repository(repository).unwrap()
    };

    let index = build();
    let retained_owner = index.clone();
    assert!(matches!(
        index.into_repository(),
        Err(IndexError::InvalidPublication(message)) if message == "index has other owners"
    ));
    drop(retained_owner);

    let index = build();
    let retained_view = index.view().unwrap();
    assert!(matches!(
        index.into_repository(),
        Err(IndexError::InvalidPublication(message)) if message == "published index has other owners"
    ));
    drop(retained_view);

    let exclusive = build();
    assert!(exclusive.into_repository().is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

fn all_query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 3_000,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn write_large_jsonl(root: &std::path::Path, label: &str) -> usize {
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    let mut records = 0usize;
    while source.len() <= 9 * 1024 * 1024 {
        source.push_str(&format!(
            "{{\"id\":\"r{records:06}\",\"text\":\"{label} {records}\"}}\n"
        ));
        records += 1;
    }
    std::fs::write(root.join("data/large.jsonl"), source).unwrap();
    records
}

fn write_large_relation_jsonl(root: &std::path::Path) -> usize {
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    let mut records = 0usize;
    while source.len() <= 9 * 1024 * 1024 {
        source.push_str(&format!(
            "{{\"id\":\"r{records:06}\",\"entry\":\"r{records:06}\",\"text\":\"final drain {records}\"}}\n"
        ));
        records += 1;
    }
    std::fs::write(root.join("data/large.jsonl"), source).unwrap();
    records
}

#[test]
fn one_large_jsonl_file_can_produce_more_than_the_segment_job_window() {
    let root = std::env::temp_dir().join(format!(
        "objekte-large-jsonl-segments-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = write_large_jsonl(&root, "large framed record");
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let task = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            build_workers: std::num::NonZeroUsize::new(1),
            segment_policy: crate::native_segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let segments = view.segments(crate::model::CollectionId(0)).unwrap();
    assert!(segments.len() > 2);
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.index.records.len())
            .sum::<usize>(),
        records
    );
    assert_eq!(view.search(&all_query()).unwrap().count, Some(records));
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn large_file_finalizes_private_segments_before_eof() {
    let root = std::env::temp_dir().join(format!(
        "objekte-private-segment-before-eof-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = write_large_jsonl(&root, "private segment");
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let gate = std::sync::Arc::new(crate::repository::SegmentJobGate::new());
    let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
    let thread_gate = std::sync::Arc::clone(&gate);
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        Repository::build_incremental_with_sources_gated(
            worker_root,
            vec![definition],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(1),
                segment_policy: crate::native_segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(1).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                ..Default::default()
            },
            SourceRegistry::builtin(),
            |index| {
                catalogue_tx.send(index.clone()).unwrap();
                Ok(())
            },
            |_| Ok(()),
            || false,
            thread_gate,
        )
    });
    let catalogue = catalogue_rx.recv().unwrap();
    gate.wait_for_workers(1);
    let before_release = catalogue.view().unwrap();
    assert!(
        before_release
            .segments(crate::model::CollectionId(0))
            .unwrap()
            .is_empty()
    );
    gate.release();
    let index = worker.join().unwrap().unwrap();
    let view = index.view().unwrap();
    let result = view.search(&all_query()).unwrap();
    assert_eq!(result.count, Some(records));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn private_segment_window_is_bounded_for_one_and_multiple_workers() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let root = std::env::temp_dir().join(format!(
        "objekte-private-segment-window-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = write_large_jsonl(&root, "bounded private segment");
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    for workers in [1usize, 3] {
        let publications = std::sync::Arc::new(AtomicUsize::new(0));
        let callback_publications = std::sync::Arc::clone(&publications);
        let index = Repository::build_incremental_with_sources(
            root.clone(),
            vec![definition.clone()],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(workers),
                segment_policy: crate::native_segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(1).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                ..Default::default()
            },
            SourceRegistry::builtin(),
            |_| Ok(()),
            move |_| {
                callback_publications.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            || false,
        )
        .unwrap();
        let repo = index.into_repository().unwrap();
        let window = workers.saturating_mul(2);
        assert!(records > window + 3);
        assert_eq!(repo.collections()[0].generation.records.len(), records);
        assert_eq!(publications.load(Ordering::Relaxed), 1);
        // Entered segment jobs can overlap while waiting to finish, so this
        // counter is not a worker-thread count. The scheduler's admission
        // bound applies to outstanding jobs, not to that broader job activity.
        assert!(repo.startup().build_peak_segment_jobs >= 1);
        assert!(repo.startup().build_peak_segment_window <= window);
        assert!(repo.startup().build_peak_segment_window >= 1);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn late_private_invalidation_and_worker_failure_publish_nothing() {
    for collection_name in [
        "__test_invalidate_after_private_finalization__",
        "__test_private_segment_failure__",
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-private-segment-failure-{}-{collection_name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        write_large_jsonl(&root, "private failure");
        let definition =
            CollectionDefinition::folder(collection_name, "data", SourceFormat::JsonLines);
        let (catalogue_tx, catalogue_rx) = mpsc::channel();
        let result = Repository::build_incremental_with_sources(
            root.clone(),
            vec![definition],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(2),
                segment_policy: crate::native_segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(1).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                inference: InferenceConfig {
                    mode: InferenceMode::AutoIndex,
                    ..Default::default()
                },
                ..Default::default()
            },
            SourceRegistry::builtin(),
            |index| {
                catalogue_tx.send(index.clone()).unwrap();
                Ok(())
            },
            |_| panic!("failed private group must not publish"),
            || false,
        );
        let error = result
            .err()
            .expect("failed private group returned an index");
        let error = format!("{error:?}");
        if collection_name == "__test_invalidate_after_private_finalization__" {
            assert!(error.contains("injected late"), "{error}");
        } else {
            assert!(
                error.contains("injected private segment finalization failure"),
                "{error}"
            );
        }
        let catalogue = catalogue_rx.recv().unwrap();
        let report = catalogue
            .view()
            .unwrap()
            .inference_report(collection_name)
            .cloned();
        assert!(
            report.as_ref().is_none_or(|report| {
                report.eligible_records == 0
                    && report.observed_paths == 0
                    && report.admitted_paths == 0
                    && report.fields.is_empty()
            }),
            "{report:?}"
        );
        assert!(
            catalogue
                .view()
                .unwrap()
                .segments(crate::model::CollectionId(0))
                .unwrap()
                .is_empty(),
            "{collection_name}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn private_final_drain_consumes_worker_error_before_returning() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ReleasePrivateJobs(std::sync::Arc<crate::repository::SegmentJobGate>);
    impl Drop for ReleasePrivateJobs {
        fn drop(&mut self) {
            self.0.release_all_private_jobs();
        }
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-private-final-drain-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = write_large_relation_jsonl(&root);
    assert!(records > 2);
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["entry".into()],
            kind: LogicalType::Id,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .register_relation(crate::relation::RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "data".into(),
            cardinality: crate::relation::RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let gate = std::sync::Arc::new(crate::repository::SegmentJobGate::new());
    gate.configure_private_final_drain_failure(0, [0, 1]);
    gate.release();
    let _guard = ReleasePrivateJobs(std::sync::Arc::clone(&gate));
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
    let publications = std::sync::Arc::new(AtomicUsize::new(0));
    let worker = std::thread::spawn({
        let root = root.clone();
        let gate = std::sync::Arc::clone(&gate);
        let publications = std::sync::Arc::clone(&publications);
        move || {
            let result = Repository::build_incremental_with_sources_gated(
                root,
                vec![definition],
                RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(4),
                    segment_policy: crate::native_segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(1).unwrap(),
                        max_files: std::num::NonZeroUsize::new(1).unwrap(),
                    },
                    inference: InferenceConfig {
                        mode: InferenceMode::AutoIndex,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                SourceRegistry::builtin(),
                |index| {
                    catalogue_tx.send(index.clone()).unwrap();
                    Ok(())
                },
                move |_| {
                    publications.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
                || false,
                gate,
            );
            result_tx.send(result).unwrap();
        }
    });
    let catalogue = catalogue_rx.recv().unwrap();
    let timeout = std::time::Duration::from_secs(10);
    assert!(gate.wait_for_private_job(0, timeout));
    assert!(gate.wait_for_private_job(1, timeout));
    assert!(gate.wait_for_private_final_drain(timeout));

    gate.release_private_job(0);
    assert!(gate.wait_for_private_error(timeout));
    assert!(
        matches!(
            result_rx.recv_timeout(std::time::Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "the build returned before the remaining worker completion drained"
    );

    gate.release_private_job(1);
    let result = result_rx
        .recv_timeout(timeout)
        .expect("build did not finish after draining private completions");
    let error = match result {
        Ok(_) => panic!("failed private group published an index"),
        Err(error) => error,
    };
    let error = format!("{error:?}");
    assert!(
        error.contains("injected final-drain private segment failure"),
        "{error}"
    );
    worker.join().unwrap();
    assert_eq!(publications.load(Ordering::Relaxed), 0);
    let (completions, errors, outstanding) = gate.private_drain_snapshot();
    assert_eq!(errors, 1);
    assert!(completions >= 2, "{completions}");
    assert_eq!(outstanding, Some(0));

    let view = catalogue.view().unwrap();
    let available = view.search_available(&all_query()).unwrap();
    assert!(available.result.hits.is_empty());
    assert_eq!(available.result.count, None);
    assert!(
        view.segments(crate::model::CollectionId(0))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        view.repository().collections()[0].generation.records.len(),
        0
    );
    assert!(
        view.repository().collections()[0]
            .relations
            .iter()
            .all(|relation| {
                relation.edge_count() == 0
                    && relation.present().is_empty()
                    && relation.unresolved_count() == 0
            })
    );
    let report = view.inference_report("data").cloned();
    assert!(
        report.as_ref().is_none_or(|report| {
            report.eligible_records == 0
                && report.observed_paths == 0
                && report.admitted_paths == 0
                && report.fields.is_empty()
        }),
        "{report:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn private_work_cancels_while_outstanding_and_after_validation() {
    use std::sync::atomic::{AtomicBool, Ordering};

    for after_validation in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "objekte-private-segment-cancel-{}-{after_validation}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        write_large_jsonl(&root, "private cancellation");
        let collection_name = if after_validation {
            "__test_cancel_after_validation__"
        } else {
            "__test_cancel_while_outstanding__"
        };
        let definition =
            CollectionDefinition::folder(collection_name, "data", SourceFormat::JsonLines);
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let gate = std::sync::Arc::new(crate::repository::SegmentJobGate::new());
        let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
        let worker = std::thread::spawn({
            let root = root.clone();
            let cancelled = std::sync::Arc::clone(&cancelled);
            let gate = std::sync::Arc::clone(&gate);
            move || {
                Repository::build_incremental_with_sources_gated(
                    root,
                    vec![definition],
                    RepositoryOptions {
                        build_workers: std::num::NonZeroUsize::new(1),
                        segment_policy: crate::native_segment::SegmentPolicy {
                            target_records: std::num::NonZeroUsize::new(1).unwrap(),
                            max_files: std::num::NonZeroUsize::new(1).unwrap(),
                        },
                        ..Default::default()
                    },
                    SourceRegistry::builtin(),
                    |index| {
                        catalogue_tx.send(index.clone()).unwrap();
                        Ok(())
                    },
                    |_| panic!("cancelled private group must not publish"),
                    || cancelled.load(Ordering::Relaxed),
                    gate,
                )
            }
        });
        let catalogue = catalogue_rx.recv().unwrap();
        gate.wait_for_workers(1);
        gate.release();
        if after_validation {
            gate.wait_for_post_validation();
        } else {
            gate.wait_for_post_submission();
        }
        cancelled.store(true, Ordering::Relaxed);
        if after_validation {
            gate.release_after_validation();
        } else {
            gate.release_after_submission();
        }
        let result = worker.join().unwrap();
        let error = result.err().expect("cancelled build returned an index");
        assert!(matches!(error, IndexError::Cancelled), "{error:?}");
        assert!(
            catalogue
                .view()
                .unwrap()
                .segments(crate::model::CollectionId(0))
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn multi_worker_segment_finalization_actually_overlaps() {
    let root = std::env::temp_dir().join(format!(
        "objekte-native-segment-overlap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..1_100 {
        std::fs::write(
            root.join(format!("data/{number:03}.json")),
            format!(r#"{{"id":"{number}","text":"segment overlap needle {number}"}}"#),
        )
        .unwrap();
    }
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let gate = std::sync::Arc::new(crate::repository::SegmentJobGate::new());
    let worker = std::thread::spawn({
        let root = root.clone();
        let gate = std::sync::Arc::clone(&gate);
        move || {
            Repository::build_incremental_with_sources_gated(
                root,
                vec![definition],
                RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(3),
                    segment_policy: crate::native_segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(2).unwrap(),
                        max_files: std::num::NonZeroUsize::new(2).unwrap(),
                    },
                    ..Default::default()
                },
                SourceRegistry::builtin(),
                |_| Ok(()),
                |_| Ok(()),
                || false,
                gate,
            )
        }
    });
    gate.wait_for_workers(2);
    gate.release();
    let index = worker.join().unwrap().unwrap();
    let repo = index.into_repository().unwrap();
    assert_eq!(repo.collections()[0].generation.records.len(), 1_100);
    assert!(repo.startup().build_peak_segment_jobs >= 2);
    assert!(repo.startup().build_peak_segment_window <= 4);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovery_and_segments_publish_without_losing_records() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-publish-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..2_050 {
        std::fs::write(
            root.join(format!("data/f{number:04}.json")),
            if number == 2_049 {
                format!(r#"{{"name":"n{number}","late":{{"rare.field":42}}}}"#)
            } else {
                format!(r#"{{"name":"n{number}"}}"#)
            },
        )
        .unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["declared".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
    let (publication_tx, publication_rx) = mpsc::sync_channel(0);
    let (resume_tx, resume_rx) = mpsc::sync_channel(0);
    let worker = std::thread::spawn({
        let root = root.clone();
        move || {
            Repository::build_incremental_with_sources(
                root,
                vec![definition],
                RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(4),
                    segment_policy: crate::native_segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(2_048).unwrap(),
                        max_files: std::num::NonZeroUsize::new(2_048).unwrap(),
                    },
                    inference: InferenceConfig {
                        mode: InferenceMode::AutoIndex,
                        ..Default::default()
                    },
                    ..RepositoryOptions::default()
                },
                SourceRegistry::builtin(),
                |index| {
                    catalogue_tx.send(index.clone()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
                |index| {
                    publication_tx.send(index.view().unwrap()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
                || false,
            )
        }
    });
    let index = catalogue_rx.recv().unwrap();
    let catalogue = index.view().unwrap();
    assert!(!catalogue.initial_construction_complete());
    assert!(
        catalogue
            .collection_definitions()
            .next()
            .unwrap()
            .schema
            .fields()
            .contains_key(&vec!["declared".into()])
    );
    assert!(catalogue.inference_report("data").is_none_or(|report| {
        report
            .fields
            .iter()
            .all(|field| field.path != ["late", "rare.field"])
    }));
    assert_eq!(catalogue.catalogue_rows().count(), 2_050);
    assert_eq!(catalogue.catalogue_row(FileId(0)).unwrap().size > 0, true);
    assert_eq!(catalogue.file_record_count(FileId(0)), None);
    assert!(catalogue.search(&all_query()).is_err());
    assert!(
        catalogue
            .get_record("data", "~generated:data/f0000.json", ReturnMode::Locator)
            .is_err()
    );
    assert!(catalogue.search_reference(&all_query()).is_err());
    assert!(catalogue.explain(&all_query()).is_err());
    let available = catalogue.search_available(&all_query()).unwrap();
    assert!(!available.complete);
    assert_eq!(available.result.count, None);
    assert_eq!(available.result.hits.len(), 0);
    resume_tx.send(()).unwrap();

    let first = publication_rx.recv().unwrap();
    assert_eq!(first.0.processed_files, 2_048);
    assert!(!first.initial_construction_complete());
    assert!(first.inference_report("data").is_none_or(|report| {
        report
            .fields
            .iter()
            .all(|field| field.path != ["late", "rare.field"])
    }));
    assert!(
        first
            .search_stream(
                &all_query(),
                crate::streaming::StreamOptions::default(),
                &crate::streaming::CancellationToken::new(),
                &mut |_| Ok(crate::streaming::StreamControl::Continue),
            )
            .is_err()
    );
    let first_results = first.search_available(&all_query()).unwrap();
    assert_eq!(first_results.result.hits.len(), 2_048);
    assert_eq!(first_results.result.count, None);
    assert!(!first_results.result.counters.match_count_complete);
    let filtered = |expression: &str| {
        let mut query = all_query();
        query.filter = Some(parse_filter(expression).unwrap());
        query
    };
    assert_eq!(
        first
            .search_available(&filtered("name:? = true"))
            .unwrap()
            .result
            .hits
            .len(),
        2_048
    );
    assert_eq!(
        first
            .search_available(&filtered("name:object = true"))
            .unwrap()
            .result
            .hits
            .len(),
        0
    );
    assert_eq!(
        first
            .search_available(&filtered("name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        0
    );
    assert_eq!(
        first
            .search_available(&filtered("name = 'n0' || name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        1
    );
    assert_eq!(
        first
            .search_available(&filtered("!(name = 'n2049')"))
            .unwrap()
            .result
            .hits
            .len(),
        2_048
    );
    let mut follow_query = all_query();
    follow_query.options.count = false;
    let first_delta = first
        .search_added_since(&catalogue, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(first_delta.result.hits.len(), 2_048);
    resume_tx.send(()).unwrap();

    let second = publication_rx.recv().unwrap();
    assert_eq!(second.0.processed_files, 2_050);
    assert!(!second.initial_construction_complete());
    let second_results = second.search_available(&all_query()).unwrap();
    assert_eq!(second_results.result.hits.len(), 2_050);
    assert_eq!(second_results.result.count, None);
    assert_eq!(
        second
            .search_available(&filtered("name:string = true"))
            .unwrap()
            .result
            .hits
            .len(),
        2_050
    );
    assert_eq!(
        second
            .search_available(&filtered("name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        1
    );
    assert_eq!(
        second
            .search_available(&filtered("name = 'n0' || name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        2
    );
    assert_eq!(
        second
            .search_available(&filtered("!(name = 'n2049')"))
            .unwrap()
            .result
            .hits
            .len(),
        2_049
    );
    let second_delta = second
        .search_added_since(&first, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(second_delta.result.hits.len(), 2);
    let coalesced = second
        .search_added_since(&catalogue, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(coalesced.result.hits.len(), 2_050);
    resume_tx.send(()).unwrap();

    let finished = worker.join().unwrap().unwrap();
    let complete = finished.view().unwrap();
    assert!(complete.initial_construction_complete());
    let inference = complete.inference_report("data").unwrap();
    assert!(inference.complete);
    assert!(inference.fields.iter().any(|field| {
        field.path == ["late", "rare.field"] && field.observed_kinds == [LogicalType::Number]
    }));
    let token = crate::streaming::CancellationToken::new();
    assert!(
        !complete
            .can_append_added_since_with_cancellation(&second, &follow_query, &token)
            .unwrap()
    );
    assert!(
        complete
            .search_added_since_bounded_with_cancellation(
                &second,
                &follow_query,
                std::num::NonZeroUsize::new(1).unwrap(),
                &token,
            )
            .unwrap()
            .is_none()
    );
    let final_results = complete.search(&all_query()).unwrap();
    assert_eq!(final_results.count, Some(2_050));
    assert_eq!(final_results.hits.len(), 2_050);
    let mut metadata = all_query();
    metadata.options.evidence = true;
    metadata.filter = Some(parse_filter("@collection.name = 'data'").unwrap());
    let metadata_result = complete.search(&metadata).unwrap();
    assert_eq!(metadata_result.count, Some(2_050));
    assert_eq!(metadata_result.counters.files_opened, 0);
    metadata.filter = Some(parse_filter("@file.created:null = true").unwrap());
    assert_eq!(complete.search(&metadata).unwrap().counters.files_opened, 0);
    let mut negative = all_query();
    negative.options.evidence = true;
    negative.filter = Some(parse_filter("!(@collection.name = 'other')").unwrap());
    let negative_result = complete.search(&negative).unwrap();
    assert_eq!(negative_result.count, Some(2_050));
    assert_eq!(negative_result.counters.files_opened, 0);
    assert_eq!(finished.status().unwrap().processed_files, 2_050);
    assert_eq!(finished.status().unwrap().build_source_reads, 2_050);
    assert_eq!(finished.status().unwrap().build_parses, 2_050);
    assert_eq!(
        final_results
            .hits
            .iter()
            .map(|hit| hit.record_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2_050
    );
    assert!(!first.initial_construction_complete());
    assert!(first.search(&all_query()).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_malformed_file_resolves_each_collection_diagnostic() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-shared-diagnostic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/broken.json"), b"{ broken").unwrap();
    let definitions = vec![
        CollectionDefinition::folder("first", "data", SourceFormat::Json),
        CollectionDefinition::folder("second", "data", SourceFormat::Json),
    ];
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let mut query = all_query();
    query.collections.clear();
    let result = view.search(&query).unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(index.status().unwrap().fallback_records, 2);
    for hit in result.hits {
        assert_eq!(hit.record_class, crate::inventory::RecordClass::RawFallback);
        let diagnostic = view
            .diagnostic(FileId(hit.file_id), hit.diagnostic_ref)
            .unwrap();
        let expected_collection = view
            .repository()
            .collections()
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
            .unwrap();
        assert_eq!(diagnostic.collection.0 as usize, expected_collection);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jsonl_valid_and_malformed_records_roundtrip_without_duplicate_ids() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/multi.jsonl"),
        b"{\"name\":\"first\"}\n{broken}\n{\"name\":\"third\"}\n",
    )
    .unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let result = view.search(&all_query()).unwrap();
    assert_eq!(result.count, Some(3));
    assert_eq!(result.hits.len(), 3);
    assert_eq!(
        result
            .hits
            .iter()
            .filter(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback)
            .count(),
        1
    );
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| &hit.record_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for hit in &result.hits {
        if hit.record_class == crate::inventory::RecordClass::RawFallback {
            assert!(
                view.diagnostic(FileId(hit.file_id), hit.diagnostic_ref)
                    .is_some()
            );
        }
    }
    let rebuilt = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let restored_result = rebuilt.view().unwrap().search(&all_query()).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| (
                &hit.record_id,
                hit.byte_start,
                hit.byte_end,
                hit.record_class
            ))
            .collect::<Vec<_>>(),
        restored_result
            .hits
            .iter()
            .map(|hit| (
                &hit.record_id,
                hit.byte_start,
                hit.byte_end,
                hit.record_class
            ))
            .collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "detailed-profiling")]
#[test]
fn large_framed_file_admission_has_component_accounting() {
    let root = std::env::temp_dir().join(format!(
        "objekte-large-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = b"{\"title\":\"".to_vec();
    source.extend(std::iter::repeat_n(b'x', 8 * 1024 * 1024 + 1));
    source.extend_from_slice(b"\"}\n");
    std::fs::write(root.join("data/large.jsonl"), source).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::JsonLines,
    )];
    let options = RepositoryOptions {
        build_workers: std::num::NonZeroUsize::new(2),
        profile_build: true,
        ..Default::default()
    };
    let task = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        options,
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let admission = &view
        .startup_stats()
        .build_profile
        .as_ref()
        .unwrap()
        .admission;
    assert!(admission.batches >= 1);
    assert_eq!(admission.files, 1);
    assert_eq!(admission.records, 1);
    assert_eq!(
        admission.total_ns,
        admission.incremental_component_ns() + admission.remainder_ns
    );
    assert_eq!(
        view.startup_stats()
            .build_profile
            .as_ref()
            .unwrap()
            .admission_by_collection["data"]
            .total_ns,
        admission.total_ns
    );
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "detailed-profiling")]
#[test]
fn incremental_admission_totals_match_each_collection() {
    let root = std::env::temp_dir().join(format!(
        "objekte-collection-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.json"), br#"{"title":"one"}"#).unwrap();
    std::fs::write(root.join("data/two.json"), br#"{"title":"two"}"#).unwrap();
    let definitions = ["first", "second"]
        .into_iter()
        .map(|name| CollectionDefinition::folder(name, "data", SourceFormat::Json))
        .collect();
    let task = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            profile_build: true,
            ..Default::default()
        },
        crate::source::SourceRegistry::builtin(),
    )
    .unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let profile = view.startup_stats().build_profile.as_ref().unwrap();
    assert_eq!(profile.admission_by_collection.len(), 2);
    assert_eq!(profile.admission.files, 4);
    assert_eq!(profile.admission.records, 4);
    assert_eq!(
        profile.admission.total_ns,
        profile
            .admission_by_collection
            .values()
            .map(|entry| entry.total_ns)
            .sum::<u64>()
    );
    assert!(profile.admission_by_collection.values().all(|entry| {
        entry.files == 2
            && entry.records == 2
            && entry.total_ns == entry.incremental_component_ns() + entry.remainder_ns
    }));
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_json_build_matches_static_inference_overlap_and_native_publication() {
    use crate::inference::{InferenceConfig, InferenceMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-direct-json-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"alpha","n":1}"#).unwrap();
    std::fs::write(root.join("data/b.json"), br#"{"name":"beta","n":2}"#).unwrap();
    std::fs::write(root.join("data/c.json"), b"{broken").unwrap();
    let definitions = ["one", "two"]
        .into_iter()
        .map(|name| CollectionDefinition::folder(name, "data", SourceFormat::Json))
        .collect::<Vec<_>>();
    let mut inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.min_nonblank_occurrences = 1;
    inference.policy.min_nonblank_records = 1;
    let options = RepositoryOptions {
        inference,
        ..Default::default()
    };
    let static_repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions.clone(),
        options,
        crate::source::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    std::fs::create_dir(root.join("index-store")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        options,
        crate::source::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    assert_eq!(index.status().unwrap().indexed_files, 3);
    assert_eq!(index.status().unwrap().live_records, 6);
    let view = index.view().unwrap();
    assert!(view.repository().collections().iter().all(|state| {
        state.generation.records.len()
            == view
                .segments(state.generation.collection)
                .unwrap()
                .iter()
                .map(|segment| segment.index.records.len())
                .sum::<usize>()
    }));
    assert_eq!(
        index.view().unwrap().repository().build_parses(),
        static_repo.build_parses()
    );
    let query = Query {
        collections: vec!["one".into(), "two".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let expected = static_repo.search(&query).unwrap();
    let check = |index: &Index| {
        let view = index.view().unwrap();
        let actual = view.search(&query).unwrap();
        assert_eq!(actual.count, expected.count);
        let hits = |result: crate::repository::SearchResult| {
            let mut hits = result
                .hits
                .into_iter()
                .map(|hit| {
                    (
                        hit.collection,
                        hit.record_id,
                        hit.file_path,
                        hit.file_status,
                    )
                })
                .collect::<Vec<_>>();
            hits.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
            hits
        };
        assert_eq!(hits(actual), hits(expected.clone()));
        for name in ["one", "two"] {
            assert_eq!(
                serde_json::to_value(view.repository().inference_report(name).unwrap()).unwrap(),
                serde_json::to_value(static_repo.inference_report(name).unwrap()).unwrap()
            );
            assert_eq!(
                serde_json::to_value(view.repository().inference_index_diagnostics(name).unwrap())
                    .unwrap(),
                serde_json::to_value(static_repo.inference_index_diagnostics(name).unwrap())
                    .unwrap()
            );
        }
    };
    check(&index);
    index.shutdown().unwrap();
    let restored = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        options,
        crate::source::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    check(&restored);
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
