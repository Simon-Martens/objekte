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
use crate::query::options::ReturnMode;
use crate::query::{Query, QueryOptions, parse_filter};
use crate::schema::FieldDefinition;
use crate::source::test_support::LINE_READER;
use crate::source::{ExtractedRecord, SourceFormat, SourceReader};

struct CountingLineReader {
    parses: Arc<AtomicUsize>,
}

impl SourceReader for CountingLineReader {
    fn name(&self) -> &'static str {
        "counting-lines"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        LINE_READER.frame(file)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        LINE_READER.parse(bytes)
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
            root.join(format!("data/{name}.counted-lines")),
            format!("(\"id\"=\"{name}\",\"rank\"={rank})\n"),
        )
        .unwrap();
    }
    let parses = Arc::new(AtomicUsize::new(0));
    let mut sources = crate::source::test_support::fixture_registry();
    sources
        .register(
            "counted-lines",
            1,
            "counted-lines",
            Arc::new(CountingLineReader {
                parses: Arc::clone(&parses),
            }),
        )
        .unwrap();
    let mut definition =
        CollectionDefinition::folder("data", "data", SourceFormat::registered("counted-lines"));
    definition.identity =
        crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
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
    std::fs::write(root.join("data/a.counted-lines"), b"(\"id\"=\"a\")\n").unwrap();
    let parses = Arc::new(AtomicUsize::new(0));
    let mut sources = crate::source::test_support::fixture_registry();
    sources
        .register(
            "counted-lines",
            1,
            "counted-lines",
            Arc::new(CountingLineReader {
                parses: Arc::clone(&parses),
            }),
        )
        .unwrap();
    let definition =
        CollectionDefinition::folder("data", "data", SourceFormat::registered("counted-lines"));
    let oversized = NonZeroUsize::new(u32::MAX as usize + 1).unwrap();
    for (policy, expected) in [
        (
            crate::index::segment::SegmentPolicy {
                target_records: oversized,
                max_files: NonZeroUsize::new(1).unwrap(),
            },
            "segment target exceeds the supported local-slot range",
        ),
        (
            crate::index::segment::SegmentPolicy {
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
fn completed_generation_releases_provisional_inference_observation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-provisional-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, value) in [("a.row", "A"), ("b.row", "B"), ("c.row", "A")] {
        std::fs::write(
            root.join("data").join(name),
            fixture_value!({ "value" => (value) }).fixture_text(),
        )
        .unwrap();
    }
    let mut provisional = None;
    let index = Repository::build_incremental_with_sources(
        root.clone(),
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
        crate::index::storage::default_factory(),
        |_| Ok(()),
        |index| {
            let view = index.view()?;
            if view.estimated_construction_observation_bytes() > 0 {
                provisional = Some(view);
            }
            Ok(())
        },
        || false,
    )
    .unwrap();
    let provisional = provisional.expect("initial publication exposed no provisional view");
    let provisional_bytes = provisional.estimated_construction_observation_bytes();
    assert!(provisional_bytes > 0);
    let published = Arc::clone(&provisional.0.segment_group(0).unwrap()[0]);
    let payload = Arc::clone(&published.index);
    let weak_observation = Arc::downgrade(
        published
            .observation
            .as_ref()
            .expect("provisional segment lost its observation"),
    );
    drop(published);

    let completed = index.view().unwrap();
    assert_eq!(completed.estimated_construction_observation_bytes(), 0);
    assert!(Arc::ptr_eq(
        &completed.0.segment_group(0).unwrap()[0].index,
        &payload
    ));
    drop(provisional);
    assert!(weak_observation.upgrade().is_none());
    drop(completed);
    index.shutdown().unwrap();
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
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let definition = crate::source::test_support::document_folder("data", "data");
    let build = || {
        let repository = Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            crate::source::test_support::fixture_registry(),
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

fn write_large_lines(root: &std::path::Path, label: &str) -> usize {
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    let mut records = 0usize;
    while source.len() <= 9 * 1024 * 1024 {
        source.push_str(&format!(
            "(\"id\"=\"r{records:06}\",\"text\"=\"{label} {records}\")\n"
        ));
        records += 1;
    }
    std::fs::write(root.join("data/large.rows"), source).unwrap();
    records
}

fn write_large_relation_lines(root: &std::path::Path) -> usize {
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source = String::new();
    let mut records = 0usize;
    while source.len() <= 9 * 1024 * 1024 {
        source.push_str(&format!(
            "(\"id\"=\"r{records:06}\",\"entry\"=\"r{records:06}\",\"text\"=\"final drain {records}\")\n"
        ));
        records += 1;
    }
    std::fs::write(root.join("data/large.rows"), source).unwrap();
    records
}

#[test]
fn one_large_lines_file_can_produce_more_than_the_segment_job_window() {
    let root = std::env::temp_dir().join(format!(
        "objekte-large-lines-segments-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let records = write_large_lines(&root, "large framed record");
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    let task = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            build_workers: std::num::NonZeroUsize::new(1),
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(256).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .unwrap();
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    let segments = view.segments(crate::model::CollectionId(0)).unwrap();
    assert!(segments.len() > 2);
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.native_payload().records.len())
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
    let records = write_large_lines(&root, "private segment");
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    let gate = std::sync::Arc::new(crate::construction::segment_jobs::SegmentJobGate::new());
    let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
    let thread_gate = std::sync::Arc::clone(&gate);
    let worker_root = root.clone();
    let worker = std::thread::spawn(move || {
        Repository::build_incremental_with_sources_gated(
            worker_root,
            vec![definition],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(1),
                segment_policy: crate::index::segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(256).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
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
    let records = write_large_lines(&root, "bounded private segment");
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    for workers in [1usize, 3] {
        let publications = std::sync::Arc::new(AtomicUsize::new(0));
        let callback_publications = std::sync::Arc::clone(&publications);
        let index = Repository::build_incremental_with_sources(
            root.clone(),
            vec![definition.clone()],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(workers),
                segment_policy: crate::index::segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(256).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
            crate::index::storage::default_factory(),
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
        write_large_lines(&root, "private failure");
        let definition = CollectionDefinition::folder(
            collection_name,
            "data",
            crate::source::test_support::line_format(),
        );
        let (catalogue_tx, catalogue_rx) = mpsc::channel();
        let result = Repository::build_incremental_with_sources(
            root.clone(),
            vec![definition],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(2),
                segment_policy: crate::index::segment::SegmentPolicy {
                    target_records: std::num::NonZeroUsize::new(256).unwrap(),
                    max_files: std::num::NonZeroUsize::new(1).unwrap(),
                },
                inference: InferenceConfig {
                    mode: InferenceMode::AutoIndex,
                    ..Default::default()
                },
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
            crate::index::storage::default_factory(),
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

    struct ReleasePrivateJobs(std::sync::Arc<crate::construction::segment_jobs::SegmentJobGate>);
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
    let records = write_large_relation_lines(&root);
    assert!(records > 2);
    let mut definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
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
        .register_relation(crate::schema::relations::RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "data".into(),
            cardinality: crate::schema::relations::RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let gate = std::sync::Arc::new(crate::construction::segment_jobs::SegmentJobGate::new());
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
                    segment_policy: crate::index::segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(256).unwrap(),
                        max_files: std::num::NonZeroUsize::new(1).unwrap(),
                    },
                    inference: InferenceConfig {
                        mode: InferenceMode::AutoIndex,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                crate::source::test_support::fixture_registry(),
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
        write_large_lines(&root, "private cancellation");
        let collection_name = if after_validation {
            "__test_cancel_after_validation__"
        } else {
            "__test_cancel_while_outstanding__"
        };
        let definition = CollectionDefinition::folder(
            collection_name,
            "data",
            crate::source::test_support::line_format(),
        );
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let gate = std::sync::Arc::new(crate::construction::segment_jobs::SegmentJobGate::new());
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
                        segment_policy: crate::index::segment::SegmentPolicy {
                            target_records: std::num::NonZeroUsize::new(256).unwrap(),
                            max_files: std::num::NonZeroUsize::new(1).unwrap(),
                        },
                        ..Default::default()
                    },
                    crate::source::test_support::fixture_registry(),
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
            root.join(format!("data/{number:03}.row")),
            format!(r#"("id"="{number}","text"="segment overlap needle {number}")"#),
        )
        .unwrap();
    }
    let definition = crate::source::test_support::document_folder("data", "data");
    let gate = std::sync::Arc::new(crate::construction::segment_jobs::SegmentJobGate::new());
    let worker = std::thread::spawn({
        let root = root.clone();
        let gate = std::sync::Arc::clone(&gate);
        move || {
            Repository::build_incremental_with_sources_gated(
                root,
                vec![definition],
                RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(3),
                    segment_policy: crate::index::segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(512).unwrap(),
                        max_files: std::num::NonZeroUsize::new(512).unwrap(),
                    },
                    ..Default::default()
                },
                crate::source::test_support::fixture_registry(),
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
            root.join(format!("data/f{number:04}.row")),
            if number == 2_049 {
                format!(r#"("name"="n{number}","late"=("rare.field"=42))"#)
            } else {
                format!(r#"("name"="n{number}")"#)
            },
        )
        .unwrap();
    }
    let mut definition = crate::source::test_support::document_folder("data", "data");
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
                    segment_policy: crate::index::segment::SegmentPolicy {
                        target_records: std::num::NonZeroUsize::new(2_048).unwrap(),
                        max_files: std::num::NonZeroUsize::new(2_048).unwrap(),
                    },
                    inference: InferenceConfig {
                        mode: InferenceMode::AutoIndex,
                        ..Default::default()
                    },
                    ..RepositoryOptions::default()
                },
                crate::source::test_support::fixture_registry(),
                crate::index::storage::default_factory(),
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
    assert_eq!(
        catalogue.catalogue_row(FileId(0)).unwrap().unwrap().size > 0,
        true
    );
    assert_eq!(catalogue.file_record_count(FileId(0)).unwrap(), None);
    assert!(catalogue.search(&all_query()).is_err());
    assert!(
        catalogue
            .get_record("data", "~generated:data/f0000.row", ReturnMode::Locator)
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
                crate::query_execution::streaming::StreamOptions::default(),
                &crate::query_execution::streaming::CancellationToken::new(),
                &mut |_| Ok(crate::query_execution::streaming::StreamControl::Continue),
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
    let token = crate::query_execution::streaming::CancellationToken::new();
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
    std::fs::write(root.join("data/broken.row"), b"{ broken").unwrap();
    let definitions = vec![
        crate::source::test_support::document_folder("first", "data"),
        crate::source::test_support::document_folder("second", "data"),
    ];
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
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
            .diagnostic(
                FileId(hit.file.as_ref().expect("file origin").file_id.0),
                hit.diagnostic_ref,
            )
            .unwrap();
        let expected_collection = view
            .repository()
            .collections()
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
            .unwrap();
        let diagnostic = diagnostic.unwrap();
        assert_eq!(diagnostic.collection.0 as usize, expected_collection);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lines_valid_and_malformed_records_roundtrip_without_duplicate_ids() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-lines-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/multi.rows"),
        b"(\"name\"=\"first\")\n(broken)\n(\"name\"=\"third\")\n",
    )
    .unwrap();
    let definition =
        CollectionDefinition::folder("data", "data", crate::source::test_support::line_format());
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
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
                view.diagnostic(
                    FileId(hit.file.as_ref().expect("file origin").file_id.0),
                    hit.diagnostic_ref
                )
                .unwrap()
                .is_some()
            );
        }
    }
    let rebuilt = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
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
                hit.source_range().expect("file range").start,
                hit.source_range().expect("file range").end,
                hit.record_class
            ))
            .collect::<Vec<_>>(),
        restored_result
            .hits
            .iter()
            .map(|hit| (
                &hit.record_id,
                hit.source_range().expect("file range").start,
                hit.source_range().expect("file range").end,
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
    let mut source = b"(\"title\"=\"".to_vec();
    source.extend(std::iter::repeat_n(b'x', 8 * 1024 * 1024 + 1));
    source.extend_from_slice(b"\")\n");
    std::fs::write(root.join("data/large.rows"), source).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        crate::source::test_support::line_format(),
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
        crate::source::test_support::fixture_registry(),
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
    std::fs::write(root.join("data/one.row"), br#"("title"="one")"#).unwrap();
    std::fs::write(root.join("data/two.row"), br#"("title"="two")"#).unwrap();
    let definitions = ["first", "second"]
        .into_iter()
        .map(|name| crate::source::test_support::document_folder(name, "data"))
        .collect();
    let task = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            profile_build: true,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
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
fn direct_fixture_build_matches_static_inference_overlap_and_native_publication() {
    use crate::inference::{InferenceConfig, InferenceMode};

    let root = std::env::temp_dir().join(format!(
        "objekte-direct-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="alpha","n"=1)"#).unwrap();
    std::fs::write(root.join("data/b.row"), br#"("name"="beta","n"=2)"#).unwrap();
    std::fs::write(root.join("data/c.row"), b"(broken").unwrap();
    let definitions = ["one", "two"]
        .into_iter()
        .map(|name| crate::source::test_support::document_folder(name, "data"))
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
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    std::fs::create_dir(root.join("index-store")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        options,
        crate::source::test_support::fixture_registry(),
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
                .map(|segment| segment.native_payload().records.len())
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
        let hits = |result: crate::query_results::SearchResult| {
            let mut hits = result
                .hits
                .into_iter()
                .map(|hit| {
                    (
                        hit.collection,
                        hit.record_id,
                        hit.file.as_ref().expect("file origin").path.clone(),
                        hit.file.as_ref().expect("file origin").status,
                    )
                })
                .collect::<Vec<_>>();
            hits.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
            hits
        };
        assert_eq!(hits(actual), hits(expected.clone()));
        for name in ["one", "two"] {
            assert_eq!(
                bincode::serialize(view.repository().inference_report(name).unwrap()).unwrap(),
                bincode::serialize(static_repo.inference_report(name).unwrap()).unwrap()
            );
            assert_eq!(
                bincode::serialize(&view.repository().inference_index_diagnostics(name).unwrap())
                    .unwrap(),
                bincode::serialize(&static_repo.inference_index_diagnostics(name).unwrap())
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
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    check(&restored);
    restored.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_initial_file_is_hidden_until_all_bindings_are_complete() {
    struct HoldSecondBinding {
        frames: AtomicUsize,
        entered: mpsc::Sender<()>,
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        fail: bool,
    }
    impl SourceReader for HoldSecondBinding {
        fn name(&self) -> &'static str {
            "shared-binding"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            if self.frames.fetch_add(1, Ordering::SeqCst) == 1 {
                self.entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                if self.fail {
                    return Err("last file binding failed deliberately".into());
                }
            }
            LINE_READER.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            LINE_READER.parse(bytes)
        }
    }

    for (large, fail, empty) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-shared-atomic-{}-{large}-{fail}-{empty}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::create_dir_all(root.join("independent")).unwrap();
        std::fs::write(root.join("independent/solo.row"), b"()").unwrap();
        let mut bytes = b"(\"id\"=\"one\")\n(\"id\"=\"two\")\n".to_vec();
        if empty {
            bytes.clear();
        }
        if large {
            bytes.resize(9 * 1024 * 1024, b' ');
        }
        std::fs::write(root.join("shared/two.bindings"), bytes).unwrap();
        let (entered, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register(
                "shared-binding",
                1,
                "bindings",
                Arc::new(HoldSecondBinding {
                    frames: AtomicUsize::new(0),
                    entered,
                    release: std::sync::Mutex::new(exits),
                    fail,
                }),
            )
            .unwrap();
        let format = SourceFormat::registered("shared-binding");
        let task = Index::start_build(
            &root,
            vec![
                CollectionDefinition::folder("entries", "shared", format.clone()),
                CollectionDefinition::folder("references", "shared", format),
                crate::source::test_support::document_folder("solo", "independent"),
            ],
            RepositoryOptions::default(),
            sources,
        )
        .unwrap();
        let index = task.wait_catalogue().unwrap();
        arrivals
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let pinned = index.view().unwrap();
        let catalog = pinned.file_catalog().unwrap();
        let shared = catalog
            .file_by_path(std::path::Path::new("shared/two.bindings"))
            .unwrap()
            .unwrap()
            .id;
        let solo = catalog
            .file_by_path(std::path::Path::new("independent/solo.row"))
            .unwrap()
            .unwrap()
            .id;
        assert_eq!(catalog.records_for_file(solo).unwrap().len(), 1);
        let held = catalog.records_for_file(shared).unwrap();
        release.send(()).unwrap();
        assert!(held.is_empty(), "a private binding must stay invisible");
        let result = task.wait_complete();
        if fail {
            assert!(result.is_err());
            assert_eq!(index.view().unwrap().revision(), pinned.revision());
        } else {
            let complete = result.unwrap().view().unwrap();
            let catalog = complete.file_catalog().unwrap();
            assert_eq!(
                catalog
                    .records_for_file_in_collection(shared, "entries")
                    .unwrap()
                    .len(),
                if empty { 0 } else { 2 }
            );
            assert_eq!(
                catalog
                    .records_for_file_in_collection(shared, "references")
                    .unwrap()
                    .len(),
                if empty { 0 } else { 2 }
            );
            assert_eq!(
                complete.revision().0,
                pinned.revision().0 + 2,
                "one group swap followed by final build completion"
            );
        }
        assert!(catalog.records_for_file(shared).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn registered_stream_seals_private_work_before_eof_and_discards_late_failure() {
    struct PauseBeforeEof {
        paused: mpsc::Sender<()>,
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        fail: bool,
    }

    impl SourceReader for PauseBeforeEof {
        fn name(&self) -> &'static str {
            "private-stream"
        }
        fn supports_streaming_build(&self) -> bool {
            true
        }
        fn frame(&self, _bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            Err("expected registered stream".into())
        }
        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            LINE_READER.parse(bytes)
        }
        fn stream_build_chunks(
            &self,
            input: &mut dyn std::io::Read,
            _: usize,
            _: usize,
            emit: &mut dyn FnMut(crate::source::BuildChunk) -> Result<(), String>,
        ) -> Option<Result<(), String>> {
            Some((|| {
                let mut first = vec![0; 4];
                input
                    .read_exact(&mut first)
                    .map_err(|error| error.to_string())?;
                emit(crate::source::BuildChunk {
                    bytes: first,
                    ranges: vec![
                        ByteRange { start: 0, end: 1 },
                        ByteRange { start: 2, end: 3 },
                    ],
                    file_offset: 0,
                    preceding_lines: 0,
                })?;
                self.paused.send(()).map_err(|error| error.to_string())?;
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|error| error.to_string())?;
                let mut last = Vec::new();
                input
                    .read_to_end(&mut last)
                    .map_err(|error| error.to_string())?;
                if self.fail {
                    return Err("late registered framing failure".into());
                }
                emit(crate::source::BuildChunk {
                    bytes: last,
                    ranges: vec![ByteRange { start: 0, end: 1 }],
                    file_offset: 4,
                    preceding_lines: 2,
                })
            })())
        }
    }

    for fail in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "objekte-private-stream-{}-{fail}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/records.private"), b"1\n2\n3").unwrap();
        let (paused, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (catalogue, views) = mpsc::channel();
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register(
                "private-stream",
                1,
                "private",
                Arc::new(PauseBeforeEof {
                    paused,
                    release: std::sync::Mutex::new(resume),
                    fail,
                }),
            )
            .unwrap();
        let gate = Arc::new(crate::construction::segment_jobs::SegmentJobGate::new());
        gate.release();
        gate.release_after_submission();
        gate.release_after_validation();
        let worker_gate = gate.clone();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            Repository::build_incremental_with_sources_gated(
                worker_root,
                vec![CollectionDefinition::folder(
                    "entries",
                    "data",
                    SourceFormat::registered("private-stream"),
                )],
                RepositoryOptions {
                    build_workers: NonZeroUsize::new(1),
                    segment_policy: crate::index::segment::SegmentPolicy {
                        target_records: NonZeroUsize::new(2).unwrap(),
                        max_files: NonZeroUsize::new(1).unwrap(),
                    },
                    ..Default::default()
                },
                sources,
                |index| {
                    catalogue.send(index.clone()).unwrap();
                    Ok(())
                },
                |_| Ok(()),
                || false,
                worker_gate,
            )
        });
        let index = views
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let finalized = gate.wait_for_private_job(0, std::time::Duration::from_secs(3));
        let pinned = index.view().unwrap();
        release.send(()).unwrap();
        assert!(
            finalized,
            "private segment must finish before the reader reaches EOF"
        );
        assert_eq!(pinned.file_record_count(FileId(0)).unwrap(), None);
        if fail {
            assert!(worker.join().unwrap().is_err());
            assert_eq!(index.view().unwrap().revision(), pinned.revision());
        } else {
            let finished = worker.join().unwrap().unwrap();
            assert_eq!(finished.status().unwrap().live_records, 3);
            assert_eq!(finished.view().unwrap().segment_summaries().len(), 2);
        }
        assert_eq!(pinned.file_record_count(FileId(0)).unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn small_lines_partitions_follow_the_record_target_across_file_continuations() {
    let root = std::env::temp_dir().join(format!(
        "objekte-small-lines-segments-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let bytes = |start| {
        (start..start + 10)
            .map(|id| format!("(\"id\"=\"row-{id}\")\n"))
            .collect::<String>()
    };
    std::fs::write(root.join("data/initial.rows"), bytes(0)).unwrap();
    let mut definition = CollectionDefinition::folder(
        "entries",
        "data",
        crate::source::test_support::line_format(),
    );
    definition.identity =
        crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
    let index = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: NonZeroUsize::new(4).unwrap(),
                max_files: NonZeroUsize::new(1).unwrap(),
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = index.view().unwrap();
    assert_eq!(
        pinned
            .segment_summaries()
            .iter()
            .map(|segment| segment.records)
            .collect::<Vec<_>>(),
        [4, 4, 2]
    );
    let next = root.join("data/next.rows");
    std::fs::write(&next, bytes(10)).unwrap();
    let added = index.add_file(&next).unwrap();
    assert_eq!(added.indexed_records, 10);
    let after = index.view().unwrap();
    assert_eq!(
        after.file_record_count(added.file.file_id).unwrap(),
        Some(10)
    );
    assert!(
        after
            .segment_summaries()
            .iter()
            .all(|segment| segment.files == 1)
    );
    assert_eq!(
        after
            .segment_summaries()
            .iter()
            .map(|segment| segment.records)
            .sum::<usize>(),
        20
    );
    assert_eq!(pinned.file_record_count(FileId(0)).unwrap(), Some(10));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_file_batches_respect_distinct_file_limit_before_decoding() {
    let root =
        std::env::temp_dir().join(format!("objekte-small-file-limit-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for file in ["a", "b", "c"] {
        std::fs::write(root.join(format!("data/{file}.row")), b"()").unwrap();
    }
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder(
            "entries", "data",
        )],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: NonZeroUsize::new(8).unwrap(),
                max_files: NonZeroUsize::new(2).unwrap(),
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    assert_eq!(
        index
            .view()
            .unwrap()
            .segment_summaries()
            .iter()
            .map(|segment| segment.files)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn initial_publication_rechecks_symlink_eligibility_after_framing() {
    use crate::inventory::discovery::{SymlinkPolicy, discover};

    struct RelinkDuringFraming(std::path::PathBuf, &'static str);
    impl SourceReader for RelinkDuringFraming {
        fn name(&self) -> &'static str {
            "relink-test"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            use std::os::unix::fs::symlink;
            std::fs::rename(self.0.join("selected"), self.0.join(self.1)).unwrap();
            symlink(self.1, self.0.join("selected")).unwrap();
            LINE_READER.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            LINE_READER.parse(bytes)
        }
    }

    for (policy, renamed) in [
        (SymlinkPolicy::Skip, ".relocated"),
        (SymlinkPolicy::FollowWithinRoot, ".relocated"),
        (SymlinkPolicy::FollowWithinRoot, "earlier-name"),
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-relink-initial-{}-{policy:?}-{renamed}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data/selected")).unwrap();
        let relative = std::path::Path::new("data/selected/entry.relink");
        std::fs::write(root.join(relative), b"()\n").unwrap();
        let mut registry = crate::source::test_support::fixture_registry();
        registry
            .register(
                "relink-test",
                1,
                "relink",
                Arc::new(RelinkDuringFraming(root.join("data"), renamed)),
            )
            .unwrap();
        let format = SourceFormat::registered("relink-test");
        let mut definition = CollectionDefinition::folder("entries", "data", format.clone());
        definition.discovery.recursive = true;
        definition.discovery.symlinks = policy;
        let discovery = definition.discovery.clone();
        let reader = registry.resolve(&format).unwrap();
        let task = Index::start_build(
            &root,
            vec![definition],
            RepositoryOptions::default(),
            registry,
        )
        .unwrap();
        let index = task.wait_catalogue().unwrap();
        let result = task.wait_complete();
        let fresh_paths =
            discover(&root, std::path::Path::new("data"), &discovery, &reader).unwrap();
        let selected = fresh_paths.contains(&root.join(relative));
        let view = index.view().unwrap();
        let catalog = view.file_catalog().unwrap();
        let file = catalog.file_by_path(relative).unwrap().unwrap();
        let records = catalog.records_for_file(file.id).unwrap().len();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(result.is_ok(), selected, "policy {policy:?}");
        assert_eq!(records, usize::from(selected));
    }
}

#[test]
fn whole_document_ranges_respect_segment_target_in_build_add_and_replace() {
    struct WholeLines;
    impl SourceReader for WholeLines {
        fn name(&self) -> &'static str {
            "whole-lines-test"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            LINE_READER.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            LINE_READER.parse(bytes)
        }
    }

    for (count, target, populations) in
        [(10, 4, vec![4, 4, 2]), (310, 100, vec![100, 100, 100, 10])]
    {
        let root = std::env::temp_dir().join(format!(
            "objekte-whole-document-packing-{}-{count}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let initial = root.join("data/initial.whole");
        let managed = root.join("data/managed.whole");
        let document = |prefix: &str| {
            (0..count)
                .map(|id| format!("(\"id\"=\"{prefix}{id:04}\")\n"))
                .collect::<String>()
        };
        std::fs::write(&initial, document("a")).unwrap();
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register("whole-lines", 1, "whole", Arc::new(WholeLines))
            .unwrap();
        let mut definition = CollectionDefinition::folder(
            "entries",
            "data",
            SourceFormat::registered("whole-lines"),
        );
        definition.identity =
            crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
        let index = Index::start_build(
            &root,
            vec![definition],
            RepositoryOptions {
                segment_policy: crate::index::segment::SegmentPolicy {
                    target_records: NonZeroUsize::new(target).unwrap(),
                    max_files: NonZeroUsize::MIN,
                },
                ..RepositoryOptions::default()
            },
            sources,
        )
        .unwrap()
        .wait_complete()
        .unwrap();

        let check = |view: &crate::lifecycle::IndexView, path: &str, prefix: &str| {
            let catalogue = view.file_catalog().unwrap();
            let file = catalogue
                .file_by_path(std::path::Path::new(path))
                .unwrap()
                .unwrap();
            let records = catalogue.records_for_file(file.id).unwrap();
            assert_eq!(file.collections.len(), 1);
            assert_eq!(records.len(), count);
            let segment_records = view
                .segments(crate::model::CollectionId(0))
                .unwrap()
                .iter()
                .filter_map(|segment| {
                    let slot = *segment.native_payload().file_index.get(&file.id)?;
                    let live = &segment.active;
                    let count = segment.native_payload().files[slot]
                        .records
                        .iter()
                        .filter(|slot| live.contains(**slot))
                        .count();
                    (count != 0).then_some(count)
                })
                .collect::<Vec<_>>();
            assert_eq!(segment_records, populations, "{path}, target {target}");
            let query = Query {
                collections: vec!["entries".into()],
                fts: None,
                filter: None,
                options: QueryOptions {
                    limit: count * 2,
                    return_mode: ReturnMode::Locator,
                    sort: Vec::new(),
                    offset: 0,
                    count: true,
                    evidence: false,
                },
            };
            let mut ranges = view
                .search(&query)
                .unwrap()
                .hits
                .into_iter()
                .filter_map(|hit| {
                    let id = hit.record_id.to_string();
                    id.starts_with(prefix)
                        .then(|| (id, hit.source_range().unwrap()))
                })
                .collect::<Vec<_>>();
            ranges.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(ranges.len(), count);
            for (i, (id, range)) in ranges.into_iter().enumerate() {
                let width = format!("(\"id\"=\"{prefix}{i:04}\")\n").len() as u64;
                assert_eq!(id, format!("{prefix}{i:04}"));
                assert_eq!(
                    range,
                    ByteRange {
                        start: i as u64 * width,
                        end: (i as u64 + 1) * width - 1
                    }
                );
            }
        };
        check(&index.view().unwrap(), "data/initial.whole", "a");
        std::fs::write(&managed, document("b")).unwrap();
        let added = index.add_file(&managed).unwrap();
        let pinned = index.view().unwrap();
        check(&pinned, "data/managed.whole", "b");
        let replacement = document("c");
        std::fs::write(&managed, &replacement).unwrap();
        let revision =
            crate::inventory::SourceRevision::from_metadata(&std::fs::metadata(&managed).unwrap())
                .unwrap();
        index
            .update_file_from_bytes(added.file, &managed, replacement.as_bytes(), revision)
            .unwrap();
        check(&index.view().unwrap(), "data/managed.whole", "c");
        check(&pinned, "data/managed.whole", "b");
        std::fs::remove_dir_all(root).unwrap();
    }
}

struct RegisteredPackingReader {
    gate: Option<std::sync::Arc<RegisteredPackingGate>>,
    gate_only_this_binding: bool,
}

struct RegisteredPackingGate {
    entered: mpsc::SyncSender<()>,
    released: std::sync::Mutex<mpsc::Receiver<()>>,
    fail: bool,
}

impl SourceReader for RegisteredPackingReader {
    fn name(&self) -> &'static str {
        "registered-packing"
    }

    fn supports_streaming_build(&self) -> bool {
        true
    }

    fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
        Err("registered stream must use chunk framing".into())
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        LINE_READER.parse(bytes)
    }

    fn stream_build_chunks(
        &self,
        input: &mut dyn std::io::Read,
        max_bytes: usize,
        max_ranges: usize,
        emit: &mut dyn FnMut(crate::source::BuildChunk) -> Result<(), String>,
    ) -> Option<Result<(), String>> {
        use std::io::{BufRead, BufReader};

        Some((|| {
            let mut buffered = BufReader::new(input);
            if let Some(gate) = &self.gate
                && (self.gate_only_this_binding
                    || buffered
                        .fill_buf()
                        .map_err(|e| e.to_string())?
                        .starts_with(b"(\"id\"=\"held"))
            {
                gate.entered.send(()).map_err(|e| e.to_string())?;
                gate.released
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .map_err(|e| e.to_string())?;
                if gate.fail {
                    return Err("fatal final binding".into());
                }
            }
            let mut framer =
                crate::source::test_support::framing::FixtureLines::from_reader(buffered);
            while let Some(chunk) = framer.next_chunk(max_bytes, max_ranges)? {
                emit(chunk)?;
            }
            Ok(())
        })())
    }
}

fn registered_packing_definition(name: &str, reader: &str) -> CollectionDefinition {
    let mut definition =
        CollectionDefinition::folder(name, "data", SourceFormat::registered(reader));
    definition.identity =
        crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
}

#[test]
fn registered_small_files_pack_into_shared_segments_and_publications() {
    for file_limit in [8, 1] {
        let root = std::env::temp_dir().join(format!(
            "objekte-small-stream-packing-{}-{file_limit}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register(
                "packing",
                1,
                "pack",
                Arc::new(RegisteredPackingReader {
                    gate: None,
                    gate_only_this_binding: false,
                }),
            )
            .unwrap();
        for file in 0..64 {
            std::fs::write(
                root.join(format!("data/{file:02}.pack")),
                format!("(\"id\"=\"r{file:02}\")\n"),
            )
            .unwrap();
        }
        let index = Index::start_build(
            &root,
            vec![registered_packing_definition("entries", "packing")],
            RepositoryOptions {
                build_workers: Some(NonZeroUsize::new(4).unwrap()),
                segment_policy: crate::index::segment::SegmentPolicy {
                    target_records: NonZeroUsize::new(8).unwrap(),
                    max_files: NonZeroUsize::new(file_limit).unwrap(),
                },
                ..RepositoryOptions::default()
            },
            sources,
        )
        .unwrap()
        .wait_complete()
        .unwrap();
        let view = index.view().unwrap();
        assert_eq!(
            view.segments(crate::model::CollectionId(0))
                .unwrap()
                .iter()
                .map(|seg| seg.native_payload().records.len())
                .collect::<Vec<_>>(),
            vec![file_limit; 64 / file_limit]
        );
        assert_eq!(view.revision().0, 2 + (64 / file_limit) as u64);
        let catalogue = view.file_catalog().unwrap();
        for n in 0..64 {
            let path = format!("data/{n:02}.pack");
            let file = catalogue
                .file_by_path(std::path::Path::new(&path))
                .unwrap()
                .unwrap();
            assert_eq!(file.collections.len(), 1);
            assert_eq!(catalogue.records_for_file(file.id).unwrap().len(), 1);
            let id = format!("r{n:02}");
            let hit = view
                .get_record("entries", &id, ReturnMode::Locator)
                .unwrap()
                .unwrap();
            assert_eq!(hit.source_range(), Some(ByteRange { start: 0, end: 12 }));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn independent_registered_groups_publish_while_a_binding_waits() {
    use std::time::{Duration, Instant};

    let other_is_searchable = |view: &crate::lifecycle::IndexView| {
        let mut query = all_query();
        query.collections = vec!["entries".into()];
        query.options.limit = 32;
        view.search_available(&query)
            .unwrap()
            .result
            .hits
            .iter()
            .any(|hit| hit.record_id == "other0")
    };
    let padded = |prefix: &str, large: bool| {
        let mut data = Vec::new();
        for record in 0..if large { 10 } else { 1 } {
            let mut line = format!("(\"id\"=\"{prefix}{record}\")").into_bytes();
            if large {
                line.resize(950 * 1024, b' ');
            }
            data.extend(line);
            data.push(b'\n');
        }
        data
    };
    for (other_large, shared, fail, workers) in [
        (true, false, false, 4),
        (false, false, false, 4),
        (false, true, false, 4),
        (false, true, true, 4),
        (false, false, false, 1),
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-stream-independent-{}-{other_large}-{shared}-{fail}-{workers}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/held.pack"), padded("held", !shared)).unwrap();
        std::fs::write(root.join("data/other.pack"), padded("other", other_large)).unwrap();
        let (entered, waiting) = mpsc::sync_channel(1);
        let (release, released) = mpsc::channel();
        let gate = Arc::new(RegisteredPackingGate {
            entered,
            released: std::sync::Mutex::new(released),
            fail,
        });
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register(
                "packing",
                1,
                "pack",
                Arc::new(RegisteredPackingReader {
                    gate: (!shared && workers > 1).then(|| gate.clone()),
                    gate_only_this_binding: false,
                }),
            )
            .unwrap();
        let mut definitions = vec![registered_packing_definition("entries", "packing")];
        if shared {
            sources
                .register(
                    "held-packing",
                    1,
                    "pack",
                    Arc::new(RegisteredPackingReader {
                        gate: Some(gate),
                        gate_only_this_binding: true,
                    }),
                )
                .unwrap();
            let mut definition = registered_packing_definition("references", "held-packing");
            definition.discovery.include = vec!["held.pack".into()];
            definitions.push(definition);
        }
        let task = Index::start_build(
            &root,
            definitions,
            RepositoryOptions {
                build_workers: Some(NonZeroUsize::new(workers).unwrap()),
                ..RepositoryOptions::default()
            },
            sources,
        )
        .unwrap();
        let index = task.wait_catalogue().unwrap();
        let mut pinned = None;
        if workers > 1 {
            let entered = waiting.recv_timeout(Duration::from_secs(5));
            let deadline = Instant::now() + Duration::from_secs(5);
            while entered.is_ok() && Instant::now() < deadline {
                let view = index.view().unwrap();
                if other_is_searchable(&view) {
                    pinned = Some(view);
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let _ = release.send(());
            assert!(entered.is_ok(), "registered callback did not enter");
            let view = pinned
                .as_ref()
                .expect("independent group was not searchable");
            let catalogue = view.file_catalog().unwrap();
            let held = catalogue
                .file_by_path(std::path::Path::new("data/held.pack"))
                .unwrap()
                .unwrap();
            assert!(catalogue.records_for_file(held.id).unwrap().is_empty());
        }
        let result = task.wait_complete();
        assert_eq!(result.is_err(), fail);
        let final_view = index.view().unwrap();
        let catalogue = final_view.file_catalog().unwrap();
        let held = catalogue
            .file_by_path(std::path::Path::new("data/held.pack"))
            .unwrap()
            .unwrap();
        assert_eq!(
            catalogue.records_for_file(held.id).unwrap().len(),
            if fail {
                0
            } else if shared {
                2
            } else {
                10
            }
        );
        assert!(other_is_searchable(&final_view));
        if let Some(pinned) = pinned {
            assert!(
                pinned
                    .file_catalog()
                    .unwrap()
                    .records_for_file(held.id)
                    .unwrap()
                    .is_empty()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ordinary_many_file_job_respects_record_target_below_file_limit() {
    let root = std::env::temp_dir().join(format!(
        "objekte-ordinary-record-target-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..10 {
        std::fs::write(root.join(format!("data/item-{number}.row")), b"()").unwrap();
    }
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder(
            "items", "data",
        )],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: NonZeroUsize::new(4).unwrap(),
                max_files: NonZeroUsize::new(16).unwrap(),
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        view.segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .map(|segment| segment.native_payload().record_count())
            .collect::<Vec<_>>(),
        [4, 4, 2]
    );
    let mut query = all_query();
    query.collections = vec!["items".into()];
    let result = view.search(&query).unwrap();
    assert_eq!(result.count, Some(10));
    let catalogue = view.file_catalog().unwrap();
    for number in 0..10 {
        let path = format!("data/item-{number}.row");
        let file = catalogue
            .file_by_path(std::path::Path::new(&path))
            .unwrap()
            .unwrap();
        assert_eq!(catalogue.records_for_file(file.id).unwrap().len(), 1);
    }
    let file = view
        .file_handle_for_path(std::path::Path::new("data/item-0.row"))
        .unwrap()
        .unwrap();
    index.delete_file(file).unwrap();
    assert_eq!(view.search(&query).unwrap().count, Some(10));
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(9));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_cancellation_keeps_prior_group_and_drains_active_callback() {
    let root = std::env::temp_dir().join(format!("objekte-review-cancel-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/held.pack"), b"(\"id\"=\"held0\")\n").unwrap();
    std::fs::write(root.join("data/peer.pack"), b"(\"id\"=\"peer\")\n").unwrap();
    let (entered, arrivals) = mpsc::sync_channel(1);
    let (release, exits) = mpsc::channel();
    let mut sources = crate::source::test_support::fixture_registry();
    sources
        .register(
            "packing",
            1,
            "pack",
            Arc::new(RegisteredPackingReader {
                gate: Some(Arc::new(RegisteredPackingGate {
                    entered,
                    released: std::sync::Mutex::new(exits),
                    fail: false,
                })),
                gate_only_this_binding: false,
            }),
        )
        .unwrap();
    let task = Index::start_build(
        &root,
        vec![registered_packing_definition("entries", "packing")],
        review_stream_options(),
        sources,
    )
    .unwrap();
    let index = task.wait_catalogue().unwrap();
    let arrival = arrivals.recv_timeout(std::time::Duration::from_secs(5));
    let mut query = all_query();
    query.collections = vec!["entries".into()];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pinned = loop {
        let view = index.view().unwrap();
        if view
            .search_available(&query)
            .unwrap()
            .result
            .hits
            .iter()
            .any(|hit| hit.record_id == "peer")
        {
            break Some(view);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    task.cancel();
    let _ = release.send(());
    let terminal = task.wait_complete();
    arrival.unwrap();
    assert!(matches!(terminal, Err(IndexError::Cancelled)));
    assert_eq!(task.status().unwrap().phase, InitialBuildPhase::Cancelled);
    let pinned = pinned.expect("peer was visible before cancelling");
    let final_view = index.view().unwrap();
    assert_eq!(final_view.revision(), pinned.revision());
    for view in [&pinned, &final_view] {
        let result = view.search_available(&query).unwrap().result;
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["peer"]
        );
    }
    assert_eq!(
        final_view.repository().stream_budget.current(),
        Default::default()
    );
    eprintln!("cancelled: prior group retained, two views agree, zero outstanding work");
    std::fs::remove_dir_all(root).unwrap();
}

fn review_stream_options() -> RepositoryOptions {
    RepositoryOptions {
        build_workers: Some(NonZeroUsize::new(4).unwrap()),
        segment_policy: crate::index::segment::SegmentPolicy {
            target_records: NonZeroUsize::new(16).unwrap(),
            max_files: NonZeroUsize::MIN,
        },
        ..Default::default()
    }
}

#[test]
fn registered_fatal_completion_waits_for_held_sibling() {
    use std::io::Read;
    use std::sync::Mutex;
    use std::time::Duration;

    struct FatalAndHeld {
        entered: mpsc::SyncSender<u8>,
        fatal: Mutex<mpsc::Receiver<()>>,
        held: Mutex<mpsc::Receiver<()>>,
        exited: Arc<AtomicUsize>,
    }

    impl SourceReader for FatalAndHeld {
        fn name(&self) -> &'static str {
            "review-fatal-and-held"
        }
        fn supports_streaming_build(&self) -> bool {
            true
        }
        fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
            unreachable!()
        }
        fn parse(&self, _: &[u8]) -> Result<ExtractedRecord, String> {
            unreachable!()
        }
        fn stream_build_chunks(
            &self,
            input: &mut dyn Read,
            _: usize,
            _: usize,
            _: &mut dyn FnMut(crate::source::BuildChunk) -> Result<(), String>,
        ) -> Option<Result<(), String>> {
            Some((|| {
                let mut kind = [0];
                input.read_exact(&mut kind).map_err(|e| e.to_string())?;
                self.entered.send(kind[0]).map_err(|e| e.to_string())?;
                let gate = if kind[0] == b'F' {
                    &self.fatal
                } else {
                    &self.held
                };
                let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(10));
                self.exited.fetch_add(1, Ordering::Release);
                if kind[0] == b'F' {
                    Err("first meaningful fatal error".into())
                } else {
                    Ok(())
                }
            })())
        }
    }

    let root = std::env::temp_dir().join(format!("objekte-review-fatal-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/fatal.pack"), b"F").unwrap();
    std::fs::write(root.join("data/held.pack"), b"H").unwrap();
    let (entered, arrivals) = mpsc::sync_channel(2);
    let (fatal, fatal_gate) = mpsc::channel();
    let (held, held_gate) = mpsc::channel();
    let exited = Arc::new(AtomicUsize::new(0));
    let mut sources = crate::source::test_support::fixture_registry();
    sources
        .register(
            "packing",
            1,
            "pack",
            Arc::new(FatalAndHeld {
                entered,
                fatal: Mutex::new(fatal_gate),
                held: Mutex::new(held_gate),
                exited: Arc::clone(&exited),
            }),
        )
        .unwrap();
    let task = Index::start_build(
        &root,
        vec![registered_packing_definition("entries", "packing")],
        review_stream_options(),
        sources,
    )
    .unwrap();
    let index = task.wait_catalogue().unwrap();
    let before = index.view().unwrap().revision();
    let first = arrivals.recv_timeout(Duration::from_secs(5));
    let second = arrivals.recv_timeout(Duration::from_secs(5));
    let (done, terminal) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = done.send(task.wait_complete());
        });
        let _ = fatal.send(());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while exited.load(Ordering::Acquire) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        let prior = terminal.recv_timeout(Duration::from_millis(150));
        let active = index
            .view()
            .unwrap()
            .repository()
            .stream_budget
            .current()
            .active_callbacks;
        let _ = held.send(());
        assert!(
            matches!(prior, Err(mpsc::RecvTimeoutError::Timeout)),
            "terminal before sibling exit"
        );
        assert_eq!(active, 1);
        let outcome = terminal.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            matches!(outcome, Err(IndexError::Build(reason)) if reason.contains("first meaningful fatal error"))
        );
    });
    let mut entered = [first.unwrap(), second.unwrap()];
    entered.sort();
    assert_eq!(entered, *b"FH");
    assert_eq!(exited.load(Ordering::Acquire), 2);
    assert_eq!(task.status().unwrap().phase, InitialBuildPhase::Failed);
    let view = index.view().unwrap();
    assert_eq!(view.revision(), before);
    assert_eq!(
        view.repository().stream_budget.current(),
        Default::default()
    );
    eprintln!("failed: terminal delayed until two callbacks exited, zero outstanding work");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_empty_frames_keep_constant_unsealed_work() {
    use std::io::Read;
    struct HeaderReader;
    impl SourceReader for HeaderReader {
        fn name(&self) -> &'static str {
            "review-header"
        }
        fn supports_streaming_build(&self) -> bool {
            true
        }
        fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
            unreachable!()
        }
        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            LINE_READER.parse(bytes)
        }
        fn stream_build_chunks(
            &self,
            input: &mut dyn Read,
            _: usize,
            _: usize,
            emit: &mut dyn FnMut(crate::source::BuildChunk) -> Result<(), String>,
        ) -> Option<Result<(), String>> {
            Some((|| {
                let mut offset = 0;
                let mut tag = [0];
                while input.read(&mut tag).map_err(|e| e.to_string())? != 0 {
                    if tag[0] == b'!' {
                        return Err("late header-reader failure".into());
                    }
                    let (bytes, ranges) = if tag[0] == b'#' {
                        (tag.to_vec(), Vec::new())
                    } else {
                        let mut bytes = tag.to_vec();
                        input.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                        let end = bytes.len() as u64;
                        (bytes, vec![ByteRange { start: 0, end }])
                    };
                    let next = offset + bytes.len() as u64;
                    emit(crate::source::BuildChunk {
                        bytes,
                        ranges,
                        file_offset: offset,
                        preceding_lines: 0,
                    })?;
                    offset = next;
                }
                Ok(())
            })())
        }
    }

    for (headers, trailer) in [
        (64, &b""[..]),
        (30_000, &b""[..]),
        (30_000, &br#"("id"="tail")"#[..]),
        (30_000, &b"!"[..]),
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-review-headers-{}-{headers}-{}",
            std::process::id(),
            trailer.len()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let mut input = vec![b'#'; headers];
        input.extend_from_slice(trailer);
        std::fs::write(root.join("data/headers.pack"), input).unwrap();
        std::fs::write(root.join("data/other.pack"), br#"("id"="peer")"#).unwrap();
        let mut sources = crate::source::test_support::fixture_registry();
        sources
            .register("packing", 1, "pack", Arc::new(HeaderReader))
            .unwrap();
        let task = Index::start_build(
            &root,
            vec![registered_packing_definition("entries", "packing")],
            review_stream_options(),
            sources,
        )
        .unwrap();
        let index = task.wait_catalogue().unwrap();
        let catalogue_view = index.view().unwrap();
        let terminal = task.wait_complete();
        let peak = catalogue_view.repository().stream_budget.peak();
        assert!(
            peak.unsealed_contributions <= 2,
            "{headers} frames: {peak:?}"
        );
        assert!(peak.private_segments <= 1, "{peak:?}");
        eprintln!(
            "zero_ranges={headers} trailer_bytes={} unsealed_peak={} private_segment_peak={}",
            trailer.len(),
            peak.unsealed_contributions,
            peak.private_segments
        );
        assert_eq!(
            catalogue_view.repository().stream_budget.current(),
            Default::default()
        );
        let view = index.view().unwrap();
        let catalog = view.file_catalog().unwrap();
        let file = catalog
            .file_by_path(std::path::Path::new("data/headers.pack"))
            .unwrap()
            .unwrap();
        if trailer == b"!" {
            assert!(
                matches!(terminal, Err(IndexError::Build(error)) if error.contains("late header-reader failure"))
            );
            assert!(catalog.records_for_file(file.id).unwrap().is_empty());
            assert!(
                view.segments(crate::model::CollectionId(0))
                    .unwrap()
                    .iter()
                    .all(|segment| !segment.native_payload().file_index.contains_key(&file.id))
            );
        } else {
            terminal.unwrap();
            assert_eq!(task.status().unwrap().processed_files, 2);
            assert_eq!(file.collections.as_ref(), &[crate::model::CollectionId(0)]);
            assert_eq!(
                catalog.records_for_file(file.id).unwrap().len(),
                usize::from(!trailer.is_empty())
            );
            if !trailer.is_empty() {
                let result = view
                    .get_record("entries", "tail", ReturnMode::Locator)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    result.source_range(),
                    Some(ByteRange {
                        start: headers as u64,
                        end: (headers + trailer.len()) as u64
                    })
                );
            }
            assert_eq!(
                view.startup_stats().build_framing_chunks,
                (headers + usize::from(!trailer.is_empty()) + 1) as u64
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn large_registered_file_decodes_concurrently_beside_a_small_file() {
    struct OverlappingParser {
        arrival: mpsc::SyncSender<()>,
        release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl SourceReader for OverlappingParser {
        fn name(&self) -> &'static str {
            "mixed-decoder-gate"
        }
        fn supports_streaming_build(&self) -> bool {
            true
        }
        fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
            unreachable!()
        }
        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            if bytes.starts_with(b"(\"id\"=\"large") {
                self.arrival.send(()).map_err(|e| e.to_string())?;
                let (mutex, changed) = self.release.as_ref();
                let (guard, timed) = changed
                    .wait_timeout_while(
                        mutex.lock().unwrap(),
                        std::time::Duration::from_secs(10),
                        |released| !*released,
                    )
                    .unwrap();
                if timed.timed_out() && !*guard {
                    return Err("timed out waiting for large decoder release".into());
                }
            }
            LINE_READER.parse(bytes)
        }
        fn stream_build_chunks(
            &self,
            input: &mut dyn std::io::Read,
            max_bytes: usize,
            _: usize,
            emit: &mut dyn FnMut(crate::source::BuildChunk) -> Result<(), String>,
        ) -> Option<Result<(), String>> {
            let reader = RegisteredPackingReader {
                gate: None,
                gate_only_this_binding: false,
            };
            reader.stream_build_chunks(input, max_bytes, 1, emit)
        }
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-review-mixed-decoders-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/small.pack"), b"(\"id\"=\"small\")\n").unwrap();
    let mut bytes = Vec::new();
    for id in 0..9 {
        bytes.extend_from_slice(format!("(\"id\"=\"large{id}\")").as_bytes());
        bytes.resize((id + 1) * (1024 * 1024) - 1, b' ');
        bytes.push(b'\n');
    }
    std::fs::write(root.join("data/large.pack"), bytes).unwrap();
    let (arrival, entered) = mpsc::sync_channel(9);
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let mut sources = crate::source::test_support::fixture_registry();
    sources
        .register(
            "packing",
            1,
            "pack",
            Arc::new(OverlappingParser {
                arrival,
                release: Arc::clone(&release),
            }),
        )
        .unwrap();
    let task = Index::start_build(
        &root,
        vec![registered_packing_definition("entries", "packing")],
        review_stream_options(),
        sources,
    )
    .unwrap();
    let index = task.wait_catalogue().unwrap();
    let both = entered
        .recv_timeout(std::time::Duration::from_secs(5))
        .and_then(|()| entered.recv_timeout(std::time::Duration::from_secs(5)));
    let mut query = all_query();
    query.collections = vec!["entries".into()];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let small_only = loop {
        let view = index.view().unwrap();
        let hits = view.search_available(&query).unwrap().result.hits;
        if hits.len() == 1 && hits[0].record_id == "small" {
            break true;
        }
        if both.is_err() || std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    both.expect("two decoder calls for the large file must overlap");
    assert!(
        small_only,
        "small group must be visible while the large file is private"
    );
    let index = task.wait_complete().unwrap();
    let view = index.view().unwrap();
    assert_eq!(view.search(&query).unwrap().count, Some(10));
    assert_eq!(view.startup_stats().build_source_reads, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_bounded_lines_blank_frames_keep_constant_unsealed_work() {
    use sha2::{Digest, Sha256};
    use std::io::{BufWriter, Write};

    let line_size = 1024 * 1024;
    let mut blank = vec![b' '; line_size];
    blank[line_size - 1] = b'\n';
    let record = b"(\"id\"=\"tail\")\n";
    let peer_record = b"(\"id\"=\"peer\")\n";
    let mut observations = Vec::new();

    for frames in [16, 64] {
        for append_record in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "objekte-bounded-blank-{}-{frames}-{append_record}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("data")).unwrap();
            let mut main =
                BufWriter::new(std::fs::File::create(root.join("data/main.blank-lines")).unwrap());
            let mut digest = Sha256::new();
            for _ in 0..frames {
                main.write_all(&blank).unwrap();
                digest.update(&blank);
            }
            if append_record {
                main.write_all(record).unwrap();
                digest.update(record);
            }
            main.flush().unwrap();
            drop(main);
            std::fs::write(root.join("data/peer.blank-lines"), peer_record).unwrap();

            let mut sources = crate::source::test_support::fixture_registry();
            sources
                .register(
                    "bounded-blank-lines",
                    1,
                    "blank-lines",
                    Arc::new(LINE_READER),
                )
                .unwrap();
            let task = Index::start_build(
                &root,
                vec![registered_packing_definition(
                    "entries",
                    "bounded-blank-lines",
                )],
                RepositoryOptions {
                    profile_build: true,
                    ..review_stream_options()
                },
                sources,
            )
            .unwrap();
            let view = task.wait_complete().unwrap().view().unwrap();
            let stats = view.startup_stats();
            let peak = stats.build_stream_peak;
            eprintln!(
                "blank_frames={frames} tail={append_record} unsealed_peak={} framing_chunks={} framing_bytes={}",
                peak.unsealed_contributions, stats.build_framing_chunks, stats.build_framing_bytes,
            );
            observations.push((
                frames,
                append_record,
                peak.unsealed_contributions,
                stats.build_framing_chunks,
                stats.build_framing_bytes,
            ));

            let catalog = view.file_catalog().unwrap();
            let main = catalog
                .file_by_path(std::path::Path::new("data/main.blank-lines"))
                .unwrap()
                .unwrap();
            assert_eq!(main.collections.as_ref(), &[crate::model::CollectionId(0)]);
            assert_eq!(
                catalog.records_for_file(main.id).unwrap().len(),
                usize::from(append_record)
            );
            assert_eq!(
                view.file_fingerprint(main.id).unwrap(),
                Some(digest.finalize().into())
            );
            assert_eq!(task.status().unwrap().processed_files, 2);
            assert!(
                view.get_record("entries", "peer", ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
            if append_record {
                let tail = view
                    .get_record("entries", "tail", ReturnMode::Locator)
                    .unwrap()
                    .unwrap();
                let start = (frames * line_size) as u64;
                assert_eq!(
                    tail.source_range(),
                    Some(ByteRange {
                        start,
                        end: start + record.len() as u64 - 1
                    })
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    assert!(
        observations.iter().all(|entry| entry.2 <= 2),
        "{observations:?}"
    );
    for (frames, append_record, _, framing_chunks, framing_bytes) in observations {
        assert_eq!(
            framing_chunks,
            (frames + usize::from(append_record) + 1) as u64
        );
        assert_eq!(
            framing_bytes,
            (frames * line_size + usize::from(append_record) * record.len() + peer_record.len())
                as u64
        );
    }
}

use crate::source::test_support::{FixtureText, fixture_value};
