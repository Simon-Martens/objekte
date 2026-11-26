use crate::index::storage::file_catalog::FileCatalogReader;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::mutation::DeleteStrategy;
use super::reconciliation::periodic::PeriodicSignal;
use super::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::{CollectionId, FieldPath, FileId, LogicalType, RecordSlot};
use crate::query::options::ReturnMode;
use crate::query::options::{SortExpression, SortField};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::query_execution::streaming::{
    CancellationToken, ExecutionContext, ExecutionLimits, ResultScope, StreamControl, StreamEvent,
    StreamOptions, StreamOutcome,
};
use crate::repository::Repository;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::relations::{RelationCardinality, RelationDefinition};
use crate::schema::{FieldDefinition, Schema};

#[test]
fn pinned_catalogue_metadata_tracks_each_publication_independently() {
    let root = std::env::temp_dir().join(format!(
        "objekte-pinned-catalogue-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let initial_path = root.join("data/a.row");
    std::fs::write(&initial_path, br#"("name"="initial")"#).unwrap();
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.view().unwrap();

    let added_path = root.join("data/b.row");
    std::fs::write(&added_path, br#"("name"="added")"#).unwrap();
    index.add_file(&added_path).unwrap();
    let after_add = index.view().unwrap();

    let moved_path = root.join("data/c.row");
    std::fs::rename(&initial_path, &moved_path).unwrap();
    let initial_handle = after_add
        .file_handle_for_path(&initial_path)
        .unwrap()
        .unwrap();
    index.move_file(initial_handle, &moved_path).unwrap();
    let after_move = index.view().unwrap();

    let paths = |view: &crate::lifecycle::IndexView| {
        view.file_paths()
            .map(|path| path.unwrap().into_owned())
            .collect::<Vec<_>>()
    };
    let catalogue_paths = |view: &crate::lifecycle::IndexView| {
        view.catalogue_rows()
            .map(|row| row.unwrap().relative_path.into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(&initial), [std::path::PathBuf::from("data/a.row")]);
    assert_eq!(catalogue_paths(&initial), paths(&initial));
    assert_eq!(
        paths(&after_add),
        [
            std::path::PathBuf::from("data/a.row"),
            std::path::PathBuf::from("data/b.row"),
        ]
    );
    assert_eq!(catalogue_paths(&after_add), paths(&after_add));
    assert_eq!(
        paths(&after_move),
        [
            std::path::PathBuf::from("data/c.row"),
            std::path::PathBuf::from("data/b.row"),
        ]
    );
    assert_eq!(catalogue_paths(&after_move), paths(&after_move));

    assert!(
        initial
            .file_handle_for_path(&initial_path)
            .unwrap()
            .is_some()
    );
    assert!(initial.file_handle_for_path(&added_path).unwrap().is_none());
    assert!(initial.file_handle_for_path(&moved_path).unwrap().is_none());
    assert!(
        after_add
            .file_handle_for_path(&added_path)
            .unwrap()
            .is_some()
    );
    assert!(
        after_add
            .file_handle_for_path(&moved_path)
            .unwrap()
            .is_none()
    );
    assert!(
        after_move
            .file_handle_for_path(&initial_path)
            .unwrap()
            .is_none()
    );
    assert!(
        after_move
            .file_handle_for_path(&moved_path)
            .unwrap()
            .is_some()
    );

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_observation_counts_only_each_pinned_publications_live_records() {
    let root = std::env::temp_dir().join(format!(
        "objekte-pinned-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let initial_path = root.join("data/a.row");
    std::fs::write(&initial_path, br#"("name"="initial")"#).unwrap();
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.view().unwrap();

    let added_path = root.join("data/b.row");
    std::fs::write(&added_path, br#"("name"="added")"#).unwrap();
    index.add_file(&added_path).unwrap();
    let after_add = index.view().unwrap();
    let initial_handle = after_add
        .file_handle_for_path(&initial_path)
        .unwrap()
        .unwrap();
    index.delete_file(initial_handle).unwrap();
    std::fs::remove_file(&initial_path).unwrap();
    let after_delete = index.view().unwrap();

    let entries = |files: usize, source_bytes: u64, records: usize| {
        vec![("data", files, source_bytes, records)]
    };
    let initial_bytes = br#"("name"="initial")"#.len() as u64;
    let added_bytes = br#"("name"="added")"#.len() as u64;
    for (view, files, source_bytes, records, expected_entries) in [
        (&initial, 1, initial_bytes, 1, entries(1, initial_bytes, 1)),
        (
            &after_add,
            2,
            initial_bytes + added_bytes,
            2,
            entries(2, initial_bytes + added_bytes, 2),
        ),
        (&after_delete, 1, added_bytes, 1, entries(1, added_bytes, 1)),
    ] {
        let observation = view.index_observation().unwrap();
        assert_eq!(observation.files, files);
        assert_eq!(observation.source_bytes, source_bytes);
        assert_eq!(observation.records, records);
        assert_eq!(observation.collection_parts.len(), 1);
        let part = &observation.collection_parts[0];
        assert_eq!(
            (
                part.collection.as_str(),
                part.files,
                part.source_bytes,
                part.records
            ),
            expected_entries[0]
        );
    }

    let paths = |view: &crate::lifecycle::IndexView| {
        view.file_paths()
            .map(|path| path.unwrap().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(&initial), [std::path::PathBuf::from("data/a.row")]);
    assert_eq!(
        paths(&after_add),
        [
            std::path::PathBuf::from("data/a.row"),
            std::path::PathBuf::from("data/b.row"),
        ]
    );
    assert_eq!(
        paths(&after_delete),
        [std::path::PathBuf::from("data/b.row")]
    );

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn index_observation_counts_shared_files_once_globally_and_per_membership() {
    let root = std::env::temp_dir().join(format!(
        "objekte-shared-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/shared.row");
    let source = br#"("name"="shared")"#;
    std::fs::write(&path, source).unwrap();
    let index = Index::start_build(
        &root,
        vec![
            crate::source::test_support::document_folder("left", "data"),
            crate::source::test_support::document_folder("right", "data"),
        ],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    let observation = view.index_observation().unwrap();
    assert_eq!(observation.files, 1);
    assert_eq!(observation.source_bytes, source.len() as u64);
    assert_eq!(observation.records, 2);
    for (part, name) in observation.collection_parts.iter().zip(["left", "right"]) {
        assert_eq!(part.collection, name);
        assert_eq!(part.files, 1);
        assert_eq!(part.source_bytes, source.len() as u64);
        assert_eq!(part.records, 1);
        assert_eq!(
            view.file_paths_for_collection(name)
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            [std::path::Path::new("data/shared.row")]
        );
    }
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segment_observation_visits_match_pinned_collection_aggregates() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("left")).unwrap();
    std::fs::create_dir_all(root.join("right")).unwrap();
    let original = root.join("left/a.rows");
    std::fs::write(&original, b"(\"name\"=\"a\",\"items\"=[])\n").unwrap();
    std::fs::write(root.join("left/z.rows"), b"(\"name\"=\"z\",\"items\"=[])\n").unwrap();
    std::fs::write(
        root.join("right/c.rows"),
        b"(\"name\"=\"c\",\"items\"=[])\n",
    )
    .unwrap();
    let first_record = std::num::NonZeroUsize::new(1).unwrap();
    let index = Index::start_build(
        &root,
        vec![
            CollectionDefinition::folder(
                "left",
                "left",
                crate::source::test_support::line_format(),
            ),
            CollectionDefinition::folder(
                "right",
                "right",
                crate::source::test_support::line_format(),
            ),
        ],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: first_record,
                max_files: first_record,
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.view().unwrap();
    let build_empty_bitmaps = initial
        .repository()
        .collections()
        .iter()
        .map(|collection| collection.empty_array_records.serialized_size())
        .collect::<Vec<_>>();

    let verify = |view: &crate::lifecycle::IndexView| {
        let mut per_collection = vec![[0usize; 9]; 2];
        let mut visited_ids = Vec::new();
        let observation = view
            .index_observation_with_segments(|segment| {
                visited_ids.push(segment.id);
                let target = &mut per_collection[segment.collection.0 as usize];
                target[0] = target[0].saturating_add(segment.active_records);
                target[1] = target[1].saturating_add(segment.posting_fields);
                target[2] = target[2].saturating_add(segment.sort_columns);
                target[3] = target[3].saturating_add(segment.relation_count);
                target[4] = target[4].saturating_add(segment.relation_edges);
                target[5] = target[5].saturating_add(segment.relation_heap_bytes);
                target[6] = target[6].saturating_add(segment.bigram_stores);
                target[7] = target[7].saturating_add(segment.distinct_gram_keys);
                target[8] = target[8].saturating_add(segment.bigram_heap_bytes);
            })
            .unwrap();
        assert_eq!(
            visited_ids,
            view.segment_summaries()
                .into_iter()
                .map(|segment| segment.id)
                .collect::<Vec<_>>()
        );
        for (number, (part, counts)) in observation
            .collection_parts
            .iter()
            .zip(&per_collection)
            .enumerate()
        {
            assert_eq!(
                [
                    part.records,
                    part.posting_fields,
                    part.sort_columns,
                    part.relation_count,
                    part.relation_edges,
                    part.relation_heap_bytes,
                    part.bigram_stores,
                    part.distinct_gram_keys,
                    part.bigram_heap_bytes,
                ],
                *counts
            );
            assert_eq!(
                part.build_empty_array_bitmap_serialized_bytes,
                build_empty_bitmaps[number]
            );
        }
        for (aggregate, counts) in [
            (observation.records, 0),
            (observation.posting_fields, 1),
            (observation.sort_columns, 2),
            (observation.relation_count, 3),
            (observation.relation_edges, 4),
            (observation.relation_heap_bytes, 5),
            (observation.bigram_stores, 6),
            (observation.distinct_gram_keys, 7),
            (observation.bigram_heap_bytes, 8),
        ] {
            assert_eq!(
                aggregate,
                per_collection
                    .iter()
                    .map(|part| part[counts])
                    .sum::<usize>()
            );
        }
        observation
    };
    let initial_observation = verify(&initial);
    let initial_segment_count = initial.segment_summaries().len();
    assert!(initial_segment_count >= 2);

    let added_path = root.join("left/b.rows");
    std::fs::write(&added_path, b"(\"name\"=\"d\",\"items\"=[])\n").unwrap();
    index.add_file(&added_path).unwrap();
    let after_add = index.view().unwrap();
    verify(&after_add);
    assert!(after_add.segment_summaries().len() > initial_segment_count);
    let initial_handle = after_add.file_handle_for_path(&original).unwrap().unwrap();
    index.delete_file(initial_handle).unwrap();
    std::fs::remove_file(original).unwrap();
    let after_delete = index.view().unwrap();
    verify(&after_delete);

    assert_eq!(
        format!("{:?}", verify(&initial)),
        format!("{initial_observation:?}")
    );
    assert_eq!(
        initial
            .index_observation()
            .unwrap()
            .collection_parts
            .iter()
            .map(|part| part.build_empty_array_bitmap_serialized_bytes)
            .collect::<Vec<_>>(),
        build_empty_bitmaps
    );

    drop(after_delete);
    drop(after_add);
    drop(initial);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn benchmark_locator_sample_follows_live_records_after_source_deletion() {
    let root = std::env::temp_dir().join(format!(
        "objekte-live-locator-sample-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    std::fs::write(root.join("data/b.row"), br#"("name"="B")"#).unwrap();
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.view().unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let first = initial.search(&query).unwrap().hits.remove(0);
    assert!(
        initial
            .get_record(&first.collection, &first.record_id, ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    let handle = initial
        .file_handle_for_path(&first.file.as_ref().unwrap().path)
        .unwrap()
        .unwrap();
    index.delete_file(handle).unwrap();

    let current = index.view().unwrap();
    let second = current.search(&query).unwrap().hits.remove(0);
    assert_ne!(second.record_id, first.record_id);
    assert!(
        current
            .get_record(&second.collection, &second.record_id, ReturnMode::Locator)
            .unwrap()
            .is_some()
    );

    drop(initial);
    drop(current);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sparse_byte_trigger_coalesces_with_revision_hysteresis() {
    let root = std::env::temp_dir().join(format!(
        "objekte-size-maintenance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/initial.row"), br#"("name"="initial")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    for number in 0..2 {
        let path = root.join(format!("data/new-{number}.row"));
        std::fs::write(&path, format!("(\"name\"=\"new-{number}\")")).unwrap();
        index.add_file(&path).unwrap();
    }
    let mut staged = (*index.view().unwrap().0).clone();
    assert_eq!(staged.segment_group(0).unwrap().len(), 3);
    for segment in staged.segment_group(0).unwrap().iter() {
        assert!(segment.native_payload().accounting.retained_bytes_estimated > 0);
        assert!(segment.native_payload().accounting.source_bytes_estimated > 0);
    }
    staged.make_room_for_segment_with_limits(0, 1, 16).unwrap();
    assert_eq!(staged.segment_group(0).unwrap().len(), 2);
    assert_eq!(staged.last_size_maintenance_revision, staged.revision.0);
    staged.make_room_for_segment_with_limits(0, 1, 16).unwrap();
    assert_eq!(staged.segment_group(0).unwrap().len(), 2);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retired_ratio_trigger_compacts_one_affected_shard() {
    let root = std::env::temp_dir().join(format!(
        "objekte-retired-maintenance-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..256 {
        std::fs::write(
            root.join(format!("data/{number:03}.row")),
            format!("(\"name\"=\"item-{number}\")"),
        )
        .unwrap();
    }
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    let old = pinned.0.segment_group(0).unwrap()[0].clone();
    for number in 0..64 {
        let path = root.join(format!("data/{number:03}.row"));
        let handle = index
            .view()
            .unwrap()
            .file_handle_for_path(&path)
            .unwrap()
            .unwrap();
        index.delete_file(handle).unwrap();
        std::fs::remove_file(path).unwrap();
    }
    let maintained = index.view().unwrap();
    assert_eq!(maintained.0.segment_group(0).unwrap().len(), 1);
    assert!(!Arc::ptr_eq(
        &old,
        &maintained.0.segment_group(0).unwrap()[0]
    ));
    assert_eq!(
        maintained.0.segment_group(0).unwrap()[0]
            .native_payload()
            .records
            .len(),
        192
    );
    assert_eq!(maintained.0.segment_group(0).unwrap()[0].active.len(), 192);
    assert_eq!(old.native_payload().records.len(), 256);
    let added = root.join("data/added.row");
    std::fs::write(&added, br#"("name"="new item")"#).unwrap();
    index.add_file(&added).unwrap();
    let current = index.view().unwrap();
    assert!(
        current
            .0
            .segment_group(0)
            .unwrap()
            .iter()
            .any(|segment| segment.native_payload().records.len() == 192)
    );
    assert_eq!(current.0.status().unwrap().live_records, 193);
    assert_eq!(maintained.0.status().unwrap().live_records, 192);
    assert_eq!(pinned.0.status().unwrap().live_records, 256);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture_repository(root: &std::path::Path) -> Repository {
    crate::lifecycle::Index::build_repository_for_test(
        root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap()
}

/// Keep tests that directly manipulate base slots on the legacy static
/// representation; normal construction now converts its base to partitions.
fn static_index(repository: Repository) -> Index {
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::new(repository),
    )
    .unwrap();
    Index::from_published(published)
}

#[test]
fn populated_flat_base_is_reference_only_and_rejects_the_optimized_state() {
    let root = std::env::temp_dir().join(format!(
        "objekte-populated-base-rejected-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("kind"="yes")"#).unwrap();
    let mut repository = fixture_repository(&root);
    repository.active = repository
        .collections
        .iter()
        .map(|collection| crate::inventory::ActiveRecords::all(collection.generation.records.len()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    repository.segments = vec![Arc::new(Vec::new()); repository.collections.len()];
    let index = Index::from_repository(repository).unwrap();
    let view = index.view().unwrap();
    let query = all_records();
    assert_eq!(view.search_reference(&query).unwrap().count, Some(1));
    assert_eq!(view.search(&query).unwrap().count, Some(1));
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_public_view_validates_queries_and_returns_an_empty_count() {
    let root = std::env::temp_dir().join(format!(
        "objekte-empty-public-view-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let index = Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    let mut query = all_records();
    query.options.count = true;
    let empty = view.search(&query).unwrap();
    assert_eq!(empty.count, Some(0));
    assert!(empty.hits.is_empty());
    query.options.sort = vec![SortField::simple("missing[", false)];
    assert!(view.search(&query).is_err());
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn all_records() -> Query {
    Query {
        collections: vec!["data".into()],
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
    }
}

#[test]
fn stopping_periodic_schedule_does_not_cancel_current_reconciliation() {
    use super::reconciliation::task::ReconciliationJob;
    use super::reconciliation::{ReconciliationProgress, ReconciliationState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    let root = std::env::temp_dir().join(format!(
        "objekte-periodic-stop-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let job = Arc::new(ReconciliationJob {
        cancelled: AtomicBool::new(false),
        progress: Mutex::new(ReconciliationProgress::default()),
        result: Mutex::new(None),
        complete: Condvar::new(),
        worker: Mutex::new(None),
    });
    {
        let mut maintenance = index.0.maintenance.lock().unwrap();
        maintenance.reconciliation = ReconciliationState::Running;
        maintenance.current = Some(Arc::clone(&job));
    }
    index
        .start_periodic_reconciliation(Duration::from_secs(60))
        .unwrap();
    index.stop_periodic_reconciliation().unwrap();
    assert!(!job.cancelled.load(Ordering::Acquire));
    index.0.maintenance.lock().unwrap().current = None;
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebuild_retries_after_concurrent_file_commit_without_losing_it() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rebuild-retry-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let old = index.view().unwrap();
    let b = root.join("data/b.row");
    let report = index
        .rebuild_with_before_publish(|attempt| {
            if attempt == 1 {
                std::fs::write(&b, br#"("name"="B")"#).unwrap();
                index.add_file(&b).unwrap();
            }
        })
        .unwrap();
    assert_eq!(report.attempts, 2);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    assert_eq!(report.live_records, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_rebuild_waits_and_cancelled_preparation_never_publishes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-background-rebuild-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let initial = index.view().unwrap().revision();
    let cancelled = index.rebuild_with_control(
        || true,
        |_, _| {},
        |_| panic!("cancelled candidate must never publish"),
    );
    assert_eq!(cancelled.unwrap_err(), IndexError::Cancelled);
    assert_eq!(index.view().unwrap().revision(), initial);
    let task = index.start_rebuild().unwrap();
    let super::maintenance::MaintenanceReport::Rebuild(report) = task.wait().unwrap() else {
        panic!("wrong operation")
    };
    assert_eq!(report.revision, initial.next().unwrap());
    assert_eq!(
        task.progress().unwrap().phase,
        super::maintenance::MaintenancePhase::Complete
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_direct_lookup_does_not_request_reconciliation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-lookup-reconcile-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/a.row");
    std::fs::write(&path, br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let view = index.view().unwrap();
    let id = view.search(&all_records()).unwrap().hits[0]
        .record_id
        .clone();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        view.get_record_available("data", &id, ReturnMode::Bytes)
            .unwrap(),
        crate::query_execution::runner::DirectLookupResult::SourceUnavailable(_)
    ));
    assert_eq!(
        index.status().unwrap().reconciliation,
        super::ReconciliationState::NotStarted
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn static_base_direct_lookup_uses_pinned_catalog_after_compatible_move() {
    let root = std::env::temp_dir().join(format!(
        "objekte-static-base-move-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let original = root.join("data/original.row");
    let destination = root.join("data/renamed.row");
    let bytes = br#"("id"="A","name"="alpha")"#;
    std::fs::write(&original, bytes).unwrap();
    let mut definition = crate::source::test_support::document_folder("data", "data");
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repository = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let index = static_index(repository);
    let old = index.view().unwrap();
    let handle = old.file_handle_for_path(&original).unwrap().unwrap();
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(
        moved.strategy,
        crate::lifecycle::mutation::FileMutationStrategy::ContentPreservingMove
    );
    let live = index.view().unwrap();
    let hit = live
        .get_record("data", "A", ReturnMode::Bytes)
        .unwrap()
        .unwrap();
    assert_eq!(
        hit.file.as_ref().expect("file origin").path,
        std::path::PathBuf::from("data/renamed.row")
    );
    assert_eq!(
        hit.payload,
        Some(crate::query_results::Payload::Bytes(bytes.to_vec()))
    );
    assert!(old.get_record("data", "A", ReturnMode::Bytes).is_err());
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quiescence_keeps_unavailable_lookup_without_reconciliation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-lookup-quiesce-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/a.row");
    std::fs::write(&path, br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let view = index.view().unwrap();
    let id = view.search(&all_records()).unwrap().hits[0]
        .record_id
        .clone();
    index.quiesce().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        view.get_record_available("data", &id, ReturnMode::Bytes)
            .unwrap(),
        crate::query_execution::runner::DirectLookupResult::SourceUnavailable(_)
    ));
    assert!(index.current_reconciliation_task().unwrap().is_none());
    assert_eq!(index.status().unwrap().revision, view.revision());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_view_stays_stable_when_a_new_root_is_published() {
    let root = std::env::temp_dir().join(format!(
        "objekte-published-view-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), b"(\"name\"=\"A\")").unwrap();
    let index = static_index(fixture_repository(&root));
    let clone = index.clone();
    let old = index.view().unwrap();
    let first_file = old.file_handle(FileId(0)).unwrap().unwrap();
    assert_eq!(old.revision(), IndexRevision::INITIAL);
    assert_eq!(index.status().unwrap().revision, IndexRevision::INITIAL);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));

    std::fs::write(root.join("data/b.row"), b"(\"name\"=\"B\")").unwrap();
    let new_revision = old.revision().next().unwrap();
    let prepared = PublishedIndex::from_repository(
        index.instance_id(),
        new_revision,
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert_eq!(index.status().unwrap().revision, IndexRevision::INITIAL);
    assert_eq!(
        index.publish_prepared(old.revision(), prepared).unwrap(),
        new_revision
    );
    let current = clone.view().unwrap();
    assert_eq!(current.revision(), new_revision);
    assert_eq!(current.search(&all_records()).unwrap().count, Some(2));
    assert!(
        current
            .search(&all_records())
            .unwrap()
            .hits
            .iter()
            .all(|hit| hit.generation == new_revision.0)
    );
    assert_eq!(
        current.file_version(FileId(0)).unwrap().unwrap().0,
        new_revision.0
    );
    assert_eq!(old.file_handle(FileId(0)).unwrap().unwrap(), first_file);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));
    assert_eq!(
        old.segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .map(|segment| segment.active.len())
            .sum::<u64>(),
        1
    );
    assert_eq!(old.search(&all_records()).unwrap().hits.len(), 1);

    let stale = PublishedIndex::from_repository(
        index.instance_id(),
        new_revision,
        Arc::clone(&current.0.repository),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(old.revision(), stale),
        Err(IndexError::RevisionConflict { expected, actual })
            if expected == IndexRevision::INITIAL && actual == new_revision
    ));
    assert_eq!(index.status().unwrap().revision, new_revision);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn publication_rejects_foreign_and_skipped_revisions_without_advancing_status() {
    let root = std::env::temp_dir().join(format!("objekte-pub-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), b"(\"name\"=\"A\")").unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let initial = index.status().unwrap();
    let foreign = PublishedIndex::from_repository(
        index.instance_id() + 1,
        IndexRevision(2),
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(IndexRevision::INITIAL, foreign),
        Err(IndexError::ForeignInstance)
    ));
    let skipped = PublishedIndex::from_repository(
        index.instance_id(),
        IndexRevision(3),
        Arc::new(fixture_repository(&root)),
    )
    .unwrap();
    assert!(matches!(
        index.publish_prepared(IndexRevision::INITIAL, skipped),
        Err(IndexError::InvalidPublication(_))
    ));
    assert_eq!(index.status().unwrap(), initial);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quiescence_rejects_prepared_publications_until_close_is_cancelled() {
    let root = std::env::temp_dir().join(format!(
        "objekte-quiesce-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let next_path = root.join("data/b.row");
    std::fs::write(&next_path, br#"("name"="B")"#).unwrap();
    let prepared = index.prepare_add_file(&next_path).unwrap();
    let expected = prepared.expected_revision;
    let staged = index.stage_prepared_add(prepared).unwrap();

    index.quiesce().unwrap();
    assert_eq!(
        index.publish_prepared(expected, staged.clone()),
        Err(IndexError::Cancelled)
    );
    assert_eq!(index.view().unwrap().revision(), expected);
    assert!(matches!(
        index.start_reconcile(),
        Err(IndexError::Cancelled)
    ));

    index.resume_after_quiesce().unwrap();
    assert_eq!(
        index.publish_prepared(expected, staged).unwrap(),
        expected.next().unwrap()
    );
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(2)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn periodic_tick_detects_change_after_initial_index_build() {
    let root = std::env::temp_dir().join(format!(
        "objekte-periodic-tick-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    index
        .start_periodic_reconciliation(Duration::from_secs(3600))
        .unwrap();
    std::fs::write(root.join("data/b.row"), br#"("name"="B")"#).unwrap();
    index
        .0
        .periodic
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .signal
        .send(PeriodicSignal::Tick)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let task = loop {
        if let Some(task) = index.current_reconciliation_task().unwrap() {
            break task;
        }
        assert!(
            Instant::now() < deadline,
            "periodic tick did not start a pass"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(task.wait().unwrap().added, 1);
    assert_eq!(index.status().unwrap().indexed_files, 2);
    assert!(index.periodic_reconciliation_enabled().unwrap());
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deleting_shared_multi_record_file_retires_every_collection_atomically() {
    let root = std::env::temp_dir().join(format!(
        "objekte-delete-file-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/shared.rows");
    std::fs::write(&source, b"(\"name\"=\"A\")\n(\"name\"=\"B\")\n").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![
            CollectionDefinition::folder(
                "left",
                "data",
                crate::source::test_support::line_format(),
            ),
            CollectionDefinition::folder(
                "right",
                "data",
                crate::source::test_support::line_format(),
            ),
        ],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let old = index.view().unwrap();
    let handle = old.file_handle(FileId(0)).unwrap().unwrap();
    assert_eq!(old.file_handle_for_path(&source).unwrap(), Some(handle));
    for collection in ["left", "right"] {
        assert_eq!(
            old.file_paths_for_collection(collection)
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            [std::path::Path::new("data/shared.rows")]
        );
    }
    let query = Query {
        collections: vec!["left".into(), "right".into()],
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
    let previous = old.search(&query).unwrap();
    assert_eq!(previous.count, Some(4));
    assert_eq!(old.select_exact("left", &query).unwrap().len(), 2);
    let result = index.delete_file(handle).unwrap();
    assert_eq!(result.revision, old.revision().next().unwrap());
    assert_eq!(result.retired_records, 4);
    assert_eq!(result.affected_collections, vec!["left", "right"]);
    assert_eq!(result.strategy, DeleteStrategy::Tombstone);
    assert_eq!(old.search(&query).unwrap().count, Some(4));
    let current = index.view().unwrap();
    assert!(current.file_paths_for_collection("left").next().is_none());
    assert!(current.file_paths_for_collection("right").next().is_none());
    assert_eq!(current.search(&query).unwrap().count, Some(0));
    assert_eq!(current.search_reference(&query).unwrap().count, Some(0));
    assert_eq!(current.select_exact("left", &query).unwrap().len(), 0);
    assert_eq!(current.search_analyze(&query).unwrap().0.count, Some(0));
    assert!(
        current
            .explain(&query)
            .unwrap()
            .iter()
            .all(|trace| trace.input_records == 0)
    );
    assert!(
        current
            .collection_graph()
            .unwrap()
            .nodes
            .iter()
            .all(|node| node.record_count == 0)
    );
    assert!(current.file_handle(FileId(0)).unwrap().is_none());
    assert!(current.file_handle_for_path(&source).unwrap().is_none());
    assert!(
        current
            .get_record(
                &previous.hits[0].collection,
                &previous.hits[0].record_id,
                ReturnMode::Locator
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(index.status().unwrap().indexed_files, 0);
    assert_eq!(index.status().unwrap().live_records, 0);
    assert!(matches!(
        index.delete_file(handle),
        Err(IndexError::FileConflict { .. })
    ));
    assert!(source.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_file_preparation_selects_shared_collections_without_publishing() {
    let root = std::env::temp_dir().join(format!(
        "objekte-prepare-add-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.rows"), b"(\"x\"=1)\n").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![
            CollectionDefinition::folder(
                "left",
                "data",
                crate::source::test_support::line_format(),
            ),
            CollectionDefinition::folder(
                "right",
                "data",
                crate::source::test_support::line_format(),
            ),
        ],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let before_view = index.view().unwrap();
    let prior_segments = before_view.0.segment_group(0).unwrap().len();
    let next_ordinal = before_view.0.next_segment_ordinal;
    let before = index.status().unwrap();
    assert!(matches!(
        index.prepare_add_file(std::path::Path::new("data/a.rows")),
        Err(IndexError::DestinationExists)
    ));
    let added = root.join("data/b.rows");
    std::fs::write(&added, b"(\"x\"=2)\n(\"x\"=3)\n").unwrap();
    let prepared = index.prepare_add_file(&added).unwrap();
    assert_eq!(prepared.expected_revision, before.revision);
    assert_eq!(
        prepared.configuration_identity,
        index.view().unwrap().configuration_identity()
    );
    assert_eq!(prepared.file.source.id, FileId(1));
    assert_eq!(prepared.file.collections.len(), 2);
    assert!(prepared.file.fingerprint.is_some());
    let staged = index.stage_prepared_add(prepared).unwrap();
    assert_eq!(staged.status().unwrap().indexed_files, 2);
    assert_eq!(staged.status().unwrap().live_records, 6);
    assert_eq!(staged.segment_group(0).unwrap().len(), prior_segments + 1);
    assert_eq!(staged.segment_group(1).unwrap().len(), prior_segments + 1);
    assert_eq!(
        staged.segment_group(0).unwrap().last().unwrap().id.get(),
        next_ordinal
    );
    assert_eq!(
        staged.segment_group(1).unwrap().last().unwrap().id.get(),
        next_ordinal + 1
    );
    assert_eq!(
        staged
            .catalog
            .id_for_path(std::path::Path::new("data/b.rows"))
            .unwrap(),
        Some(FileId(1))
    );
    assert_eq!(index.status().unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replacement_stages_shared_memberships_and_retires_old_slots() {
    let root = std::env::temp_dir().join(format!(
        "objekte-prepare-replace-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let old_path = root.join("data/old.rows");
    std::fs::write(&old_path, b"(\"name\"=\"a\")\n(\"name\"=\"b\")\n").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![
            CollectionDefinition::folder(
                "left",
                "data",
                crate::source::test_support::line_format(),
            ),
            CollectionDefinition::folder(
                "right",
                "data",
                crate::source::test_support::line_format(),
            ),
        ],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let prior_segments = index.view().unwrap().0.segment_group(0).unwrap().len();
    let before = index.status().unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle(FileId(0))
        .unwrap()
        .unwrap();
    let new_path = root.join("data/new.rows");
    std::fs::write(&new_path, b"(\"name\"=\"replacement\")\n").unwrap();
    let prepared = index.prepare_replace_file(handle, &new_path).unwrap();
    let staged = index.stage_prepared_replace(prepared).unwrap();
    assert_eq!(staged.revision, before.revision.next().unwrap());
    assert_eq!(staged.status().unwrap().indexed_files, 1);
    assert_eq!(staged.status().unwrap().live_records, 2);
    assert_eq!(staged.active[0].len(), 0);
    assert_eq!(staged.active[1].len(), 0);
    assert_eq!(staged.segment_group(0).unwrap().len(), prior_segments + 1);
    assert_eq!(staged.segment_group(1).unwrap().len(), prior_segments + 1);
    assert_eq!(
        staged
            .catalog
            .id_for_path(std::path::Path::new("data/old.rows"))
            .unwrap(),
        None
    );
    assert_eq!(
        staged
            .catalog
            .id_for_path(std::path::Path::new("data/new.rows"))
            .unwrap(),
        Some(FileId(0))
    );
    assert_eq!(
        staged.catalog.version(FileId(0)).unwrap().unwrap().0,
        handle.version.0 + 1
    );
    assert_eq!(index.status().unwrap(), before);
    index.publish_prepared(before.revision, staged).unwrap();
    let query = Query {
        collections: vec!["left".into(), "right".into()],
        ..all_records()
    };
    let current = index.view().unwrap();
    let result = current.search(&query).unwrap();
    assert_eq!(result.count, Some(2));
    assert!(result.hits.iter().all(|hit| {
        hit.file
            .as_ref()
            .expect("file origin")
            .path
            .ends_with("new.rows")
    }));
    for hit in &result.hits {
        assert_eq!(
            current
                .get_record(&hit.collection, &hit.record_id, ReturnMode::Locator)
                .unwrap()
                .unwrap()
                .file
                .unwrap()
                .path,
            hit.file.as_ref().expect("file origin").path
        );
    }
    assert!(matches!(
        index.prepare_replace_file(handle, &new_path),
        Err(IndexError::FileConflict { .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconciliation_applies_metadata_changes_without_revising_unchanged_files() {
    let root = std::env::temp_dir().join(format!(
        "objekte-reconcile-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let original = root.join("data/a.rows");
    std::fs::write(&original, b"(\"name\"=\"a\")\n").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.status().unwrap();
    let unchanged = index.reconcile_pass(|| false);
    assert_eq!(unchanged.examined, 1);
    assert_eq!(unchanged.unchanged, 1);
    assert!(unchanged.issues.is_empty());
    assert_eq!(index.status().unwrap(), initial);

    let added = root.join("data/b.rows");
    std::fs::write(&added, b"(\"name\"=\"b\")\n").unwrap();
    let report = index.reconcile_pass(|| false);
    assert_eq!((report.added, report.updated, report.removed), (1, 0, 0));
    assert_eq!(
        report.indexed_bytes,
        std::fs::metadata(&added).unwrap().len()
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(index.status().unwrap().indexed_files, 2);

    std::fs::write(&original, b"(\"name\"=\"replacement\")\n").unwrap();
    let report = index.reconcile_pass(|| false);
    assert_eq!((report.added, report.updated, report.removed), (0, 1, 0));
    assert_eq!(
        report.indexed_bytes,
        std::fs::metadata(&original).unwrap().len()
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(index.status().unwrap().live_records, 2);

    std::fs::remove_file(&added).unwrap();
    let report = index.reconcile_pass(|| false);
    assert_eq!((report.added, report.updated, report.removed), (0, 0, 1));
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(index.status().unwrap().indexed_files, 1);
    assert_eq!(index.status().unwrap().live_records, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_reconciliation_reports_completion_and_joins() {
    let root = std::env::temp_dir().join(format!(
        "objekte-background-reconcile-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.rows"), b"(\"name\"=\"a\")\n").unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let before = index.status().unwrap();
    let task = index.start_reconcile().unwrap();
    let report = task.wait().unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(report.issues.is_empty());
    assert!(task.is_complete().unwrap());
    assert_eq!(
        task.progress().unwrap(),
        super::reconciliation::ReconciliationProgress::from(&report)
    );
    assert_eq!(index.status().unwrap().revision, before.revision);
    assert_eq!(
        index.status().unwrap().reconciliation,
        super::ReconciliationState::Complete
    );
    assert_eq!(index.shutdown().unwrap(), Some(report));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn many_small_additions_and_replacements_stay_within_segment_budget() {
    let root = std::env::temp_dir().join(format!(
        "objekte-bounded-segments-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    for number in 0..70 {
        let relative = format!("data/file-{number}.rows");
        std::fs::write(root.join(&relative), format!("(\"number\"={number})\n")).unwrap();
        let prepared = index
            .prepare_add_file(std::path::Path::new(&relative))
            .unwrap();
        let expected = prepared.expected_revision;
        let staged = index.stage_prepared_add(prepared).unwrap();
        index.publish_prepared(expected, staged).unwrap();
    }
    assert_eq!(index.status().unwrap().indexed_files, 70);
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(70)
    );
    assert!(index.view().unwrap().0.segment_group(0).unwrap().len() <= 64);

    let relative = std::path::Path::new("data/file-0.rows");
    for number in 0..70 {
        std::fs::write(root.join(relative), format!("(\"updated\"={number})\n")).unwrap();
        let handle = index
            .view()
            .unwrap()
            .file_handle_for_path(relative)
            .unwrap()
            .unwrap();
        let prepared = index.prepare_replace_file(handle, relative).unwrap();
        let expected = prepared.expected_revision;
        let staged = index.stage_prepared_replace(prepared).unwrap();
        index.publish_prepared(expected, staged).unwrap();
    }
    assert_eq!(
        index.view().unwrap().search(&all_records()).unwrap().count,
        Some(70)
    );
    assert!(index.view().unwrap().0.segment_group(0).unwrap().len() <= 64);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retired_slots_are_excluded_before_planning_count_sort_stream_and_lookup() {
    let root = std::env::temp_dir().join(format!(
        "objekte-live-query-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/shared.rows"),
        b"(\"sequence\"=1,\"kind\"=\"yes\",\"body\"=\"needle\")\n(\"sequence\"=2,\"kind\"=\"yes\",\"body\"=\"needle\")\n(\"sequence\"=3,\"kind\"=\"yes\",\"body\"=\"needle\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join("data/other.rows"),
        b"(\"sequence\"=4,\"kind\"=\"yes\",\"body\"=\"needle\")\n(\"sequence\"=5,\"kind\"=\"yes\",\"body\"=\"needle\")\n(\"sequence\"=6,\"kind\"=\"yes\",\"body\"=\"needle\")\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    for (field, kind) in [
        ("sequence", LogicalType::Number),
        ("kind", LogicalType::String),
        ("body", LogicalType::String),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![field.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repository = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: crate::source::test_support::line_format(),
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![FieldPath::parse("sequence").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let index = static_index(repository);
    let old = index.view().unwrap();
    let collection = &old.repository().collections()[0];
    let retired = [RecordSlot(1), RecordSlot(4)];
    let retired_ids = retired
        .iter()
        .map(|slot| collection.generation.records[slot.0 as usize].id.0.clone())
        .collect::<Vec<_>>();
    let mut next = PublishedIndex::from_repository(
        index.instance_id(),
        old.revision().next().unwrap(),
        Arc::clone(&old.0.repository),
    )
    .unwrap();
    {
        let mut group = next.segment_group(0).unwrap();
        let group = Arc::make_mut(&mut group);
        let segment = Arc::make_mut(&mut group[0]);
        segment.active = old.segments(crate::model::CollectionId(0)).unwrap()[0]
            .active
            .retired(&retired)
            .unwrap();
        next.replace_segment_group(0, group).unwrap();
    }
    index.publish_prepared(old.revision(), next).unwrap();
    let live = index.view().unwrap();
    assert_eq!(old.search(&all_records()).unwrap().count, Some(6));
    assert_eq!(live.search(&all_records()).unwrap().count, Some(4));
    assert_eq!(live.count_exact(&all_records()).unwrap(), 4);
    assert_eq!(
        live.search_reference(&all_records())
            .unwrap()
            .counters
            .scanned_records,
        4
    );
    for id in retired_ids {
        assert!(
            live.get_record("data", &id, ReturnMode::Bytes)
                .unwrap()
                .is_none()
        );
        assert!(
            old.get_record("data", &id, ReturnMode::Locator)
                .unwrap()
                .is_some()
        );
    }

    let mut sorted = all_records();
    sorted.options.sort = vec![SortField::simple("sequence", false)];
    sorted.options.offset = 1;
    sorted.options.limit = 2;
    let mut exact = sorted.clone();
    exact.filter = Some(parse_filter("kind = 'yes'").unwrap());
    let mut negated = sorted.clone();
    negated.filter = Some(parse_filter("!(kind = 'no')").unwrap());
    let mut fts = sorted.clone();
    fts.fts = Some(FtsQuery {
        pattern_parameter: None,
        mode: FtsMode::Literal,
        case_sensitivity: CaseSensitivity::Sensitive,
        literal: "needle".into(),
        fields: vec![],
    });
    for query in [&sorted, &exact, &negated, &fts] {
        let optimized = live.search(query).unwrap();
        let reference = live.search_reference(query).unwrap();
        assert_eq!(optimized.count, Some(4));
        assert_eq!(optimized.hits, reference.hits);
        assert_eq!(optimized.hits.len(), 2);
        assert_eq!(live.count_exact(query).unwrap(), 4);
        let mut streamed = Vec::new();
        let outcome = live
            .search_stream_scoped(
                query,
                ResultScope::All { offset: 0 },
                StreamOptions::default(),
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { rows, .. } = event {
                        streamed.extend(rows.iter().cloned());
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        assert!(matches!(outcome, StreamOutcome::Complete(_)));
        let mut whole = query.clone();
        whole.options.offset = 0;
        whole.options.limit = 10;
        assert_eq!(streamed, live.search(&whole).unwrap().hits);
    }
    let context = || {
        ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
    };
    assert_eq!(
        live.search_with_context(&sorted, &context()).unwrap().hits,
        live.search(&sorted).unwrap().hits
    );
    assert_eq!(
        live.count_exact_with_context(&sorted, &context()).unwrap(),
        4
    );
    let mut page_rows = Vec::new();
    let page = live
        .search_stream(
            &sorted,
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    page_rows.extend(rows.iter().cloned());
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    assert!(matches!(page, StreamOutcome::Complete(_)));
    assert_eq!(page_rows, live.search(&sorted).unwrap().hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_queries_keep_retired_targets_unknown_and_skip_retired_sources() {
    let root = std::env::temp_dir().join(format!(
        "objekte-live-relations-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["targets", "sources"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (path, bytes) in [
        ("targets/a.row", r#"("id"="A","title"="Alpha")"#),
        ("targets/b.row", r#"("id"="B","title"="Beta")"#),
        ("sources/s1.row", r#"("id"="s1","entry"="A","title"="one")"#),
        ("sources/s2.row", r#"("id"="s2","entry"="B","title"="two")"#),
        (
            "sources/s3.row",
            r#"("id"="s3","entry"="A","title"="three")"#,
        ),
        (
            "sources/s4.row",
            r#"("id"="s4","entry"="B","title"="four")"#,
        ),
    ] {
        std::fs::write(root.join(path), bytes).unwrap();
    }
    let mut definitions = Vec::new();
    for name in ["targets", "sources"] {
        let mut schema = Schema::default();
        for (field, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
            schema
                .register_field(FieldDefinition {
                    path: vec![field.into()],
                    kind,
                    required: true,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        if name == "sources" {
            schema
                .register_field(FieldDefinition {
                    path: vec!["entry".into()],
                    kind: LogicalType::Id,
                    required: true,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
            schema
                .register_relation(RelationDefinition {
                    path: vec!["entry".into()],
                    target_collection: "targets".into(),
                    cardinality: RelationCardinality::One,
                    reverse_alias: Some("sources".into()),
                })
                .unwrap();
        }
        let mut definition = crate::source::test_support::document_folder(name, name);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        definition.schema = schema;
        definitions.push(definition);
    }
    let repository = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let index = static_index(repository);
    let old = index.view().unwrap();
    let mut next = PublishedIndex::from_repository(
        index.instance_id(),
        old.revision().next().unwrap(),
        Arc::clone(&old.0.repository),
    )
    .unwrap();
    for (collection, record_id) in [(0, "A"), (1, "s2")] {
        let mut group = next.segment_group(collection).unwrap();
        let group = Arc::make_mut(&mut group);
        let segment = group
            .iter()
            .position(|segment| segment.native_payload().id_lookup.contains_key(record_id))
            .unwrap();
        let segment = Arc::make_mut(&mut group[segment]);
        let slot = segment.native_payload().id_lookup[record_id];
        segment.active = segment.active.retired(&[slot]).unwrap();
        next.replace_segment_group(collection, group).unwrap();
    }
    index.publish_prepared(old.revision(), next).unwrap();
    let live = index.view().unwrap();
    let request = |collection: &str, filter: &str| Query {
        collections: vec![collection.into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for (collection, filter, expected) in [
        ("sources", "entry.title = 'Beta'", vec!["s4"]),
        ("sources", "entry.title = 'Alpha'", vec![]),
        ("sources", "!(entry.title = 'Alpha')", vec!["s4"]),
        ("sources", "title != entry.title", vec!["s4"]),
        ("targets", "@backlinks.sources.title = 'four'", vec!["B"]),
        ("targets", "@backlinks.sources.title = 'two'", vec![]),
    ] {
        let query = request(collection, filter);
        let optimized = live.search(&query).unwrap();
        let reference = live.search_reference(&query).unwrap();
        let ids = optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(optimized.hits, reference.hits, "{filter}");
        if filter == "title != entry.title" {
            assert_eq!(optimized.counters.relation_target_reads, 1);
            assert_eq!(reference.counters.relation_target_reads, 1);
        }
        assert_eq!(
            live.count_exact(&query).unwrap(),
            expected.len(),
            "{filter}"
        );
    }
    assert_eq!(
        old.search(&request("sources", "entry.title = 'Alpha'"))
            .unwrap()
            .count,
        Some(2)
    );
    let mut selected_fts = request("sources", "@record.id != 'none'");
    selected_fts.filter = None;
    selected_fts.fts = Some(FtsQuery {
        pattern_parameter: None,
        mode: FtsMode::Literal,
        case_sensitivity: CaseSensitivity::Sensitive,
        literal: "Beta".into(),
        fields: vec![crate::query::FtsField {
            path: FieldPath::parse("entry.title").unwrap(),
            binding: None,
            query_path: None,
        }],
    });
    let fts_result = live.search(&selected_fts).unwrap();
    assert_eq!(fts_result.count, Some(1));
    assert_eq!(fts_result.hits[0].record_id, "s4");
    assert_eq!(
        fts_result.hits,
        live.search_reference(&selected_fts).unwrap().hits
    );
    let mut stream_rows = Vec::new();
    let outcome = live
        .search_stream_scoped(
            &selected_fts,
            ResultScope::All { offset: 0 },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    stream_rows.extend(rows.iter().cloned());
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    assert!(matches!(outcome, StreamOutcome::Complete(_)));
    assert_eq!(stream_rows, fts_result.hits);
    let mut sorted = request("sources", "@record.id != 'none'");
    sorted.filter = None;
    sorted.options.sort = vec![SortField::computed(
        SortExpression {
            path: "entry.title".into(),
            binding: None,
            reducer: None,
        },
        false,
    )];
    let optimized = live.search(&sorted).unwrap();
    assert_eq!(optimized.hits, live.search_reference(&sorted).unwrap().hits);
    assert_eq!(optimized.count, Some(3));
    assert_eq!(
        live.get_record("targets", "A", ReturnMode::Bytes).unwrap(),
        None
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_estimate_reuses_cached_native_accounting() {
    let root = std::env::temp_dir().join(format!(
        "objekte-cached-status-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("title"="Alpha")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    super::reset_index_family_estimate_calls();
    let first = view.estimated_index_bytes();
    assert!(first > 0);
    assert_eq!(super::index_family_estimate_calls(), 0);
    let second = view.estimated_index_bytes();
    assert_eq!(second, first);
    assert_eq!(super::index_family_estimate_calls(), 0);

    std::fs::write(
        root.join("data/b.row"),
        br#"("title"="Beta","number"=7,"flag"=true)"#,
    )
    .unwrap();
    index.add_file(&root.join("data/b.row")).unwrap();
    let grown = index.view().unwrap().estimated_index_bytes();
    assert!(grown > first);
    super::reset_index_family_estimate_calls();
    let polling_view = index.view().unwrap();
    let _ = polling_view.estimated_index_bytes();
    let _ = polling_view.estimated_index_bytes();
    assert_eq!(super::index_family_estimate_calls(), 0);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_accounting_grows_with_postings_and_survives_retirement() {
    let root = std::env::temp_dir().join(format!(
        "objekte-native-accounting-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("title"="Alpha")"#).unwrap();
    std::fs::write(root.join("data/b.row"), br#"("title"="Beta")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    let total = |view: &crate::lifecycle::IndexView| {
        view.segments(CollectionId(0))
            .unwrap()
            .iter()
            .map(|segment| segment.native_payload().accounting.retained_bytes_estimated)
            .sum::<usize>()
    };
    let before = total(&view);
    assert!(before > 0);
    let first = view.segments(CollectionId(0)).unwrap()[0].clone();
    assert!(first.native_payload().accounting.index_family_bytes > 0);
    assert_eq!(
        first.native_payload().accounting.max_record_range_len,
        first.native_payload().backing.max_file_range_len()
    );
    std::fs::write(
        root.join("data/c.row"),
        br#"("title"="Gamma","alpha"="one","beta"=2,"gamma"=true)"#,
    )
    .unwrap();
    index.add_file(&root.join("data/c.row")).unwrap();
    let after = total(&index.view().unwrap());
    assert!(after > before);

    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&root.join("data/a.row"))
        .unwrap()
        .unwrap();
    index.delete_file(handle).unwrap();
    let current = index.view().unwrap();
    let retained = current
        .segments(CollectionId(0))
        .unwrap()
        .iter()
        .find(|segment| segment.id == first.id)
        .cloned()
        .expect("retired shard missing");
    assert_eq!(
        retained
            .native_payload()
            .accounting
            .retained_bytes_estimated,
        first.native_payload().accounting.retained_bytes_estimated
    );
    assert_eq!(
        retained.native_payload().accounting.index_family_bytes,
        first.native_payload().accounting.index_family_bytes
    );
    assert!(retained.active.len() < first.active.len());
    index.compact().unwrap();
    let compacted = index.view().unwrap();
    let compacted_total = total(&compacted);
    assert!(compacted_total > 0);
    assert!(
        compacted
            .segments(CollectionId(0))
            .unwrap()
            .iter()
            .all(|segment| {
                segment.native_payload().accounting.retained_bytes_estimated > 0
                    && segment.native_payload().accounting.index_family_bytes > 0
            })
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn generated_namespaces_are_rejected_by_mutation_and_reconciliation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-generated-mutation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data/nested/objekte-indexes")).unwrap();
    std::fs::create_dir_all(root.join(".objekte/indexes")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("title"="Alpha")"#).unwrap();
    std::fs::write(
        root.join("data/nested/objekte-indexes/poison.row"),
        br#"("title"="Poison")"#,
    )
    .unwrap();
    let mut definition = crate::source::test_support::document_folder("data", "data");
    definition.discovery.recursive = true;
    definition.discovery.hidden = true;
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    assert_eq!(
        index.view().unwrap().search(&all_query()).unwrap().count,
        Some(1)
    );

    let poison = root.join("data/nested/objekte-indexes/added.row");
    std::fs::write(&poison, br#"("title"="Added poison")"#).unwrap();
    assert!(index.add_file(&poison).is_err());
    assert_eq!(
        index.view().unwrap().search(&all_query()).unwrap().count,
        Some(1)
    );

    let b = root.join("data/b.row");
    std::fs::write(&b, br#"("title"="Beta")"#).unwrap();
    let added = index.add_file(&b).unwrap();
    let moved = root.join("data/nested/objekte-indexes/moved.row");
    std::fs::rename(&b, &moved).unwrap();
    assert!(index.move_file(added.file, &moved).is_err());
    std::fs::write(&b, br#"("title"="Beta changed")"#).unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&b)
        .unwrap()
        .unwrap();
    let generated_update = root.join("data/nested/objekte-indexes/updated.row");
    std::fs::write(&generated_update, br#"("title"="Updated poison")"#).unwrap();
    assert!(index.update_file(handle, &generated_update).is_err());
    let report = index.start_reconcile().unwrap().wait().unwrap();
    assert_eq!(report.added, 0);
    assert_eq!(
        index.view().unwrap().search(&all_query()).unwrap().count,
        Some(2)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_fast_paths_and_fallbacks_match_the_reference_evaluator() {
    let root = std::env::temp_dir().join(format!(
        "objekte-metadata-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.row"), br#"("title"="Alpha")"#).unwrap();
    std::fs::write(root.join("data/b.row"), br#"("title"="Beta")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    for (filter, supported) in [
        ("@file.modified >= 2000-01-01", true),
        ("@file.size >= 1", true),
        ("@file.created:null = true", false),
        ("@file.path:lower = 'data/a.row'", false),
        ("@file.path ~ 'row$'", false),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 50,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let actual = view.search(&query).unwrap();
        let reference = view.search_reference(&query).unwrap();
        assert_eq!(actual.count, reference.count, "{filter}");
        assert_eq!(actual.hits, reference.hits, "{filter}");
        if supported {
            assert_eq!(actual.counters.files_opened, 0, "{filter}");
            assert_eq!(actual.counters.record_parses, 0, "{filter}");
        }
    }
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn record_range_scheduling_boundary_preserves_results_and_cancellation() {
    fn fixture(label: &str, target: usize) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "objekte-range-boundary-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let padding = target.checked_sub(11).unwrap();
        let document = format!(r#"("text"="{}")"#, "a".repeat(padding));
        assert_eq!(document.len(), target);
        std::fs::write(root.join("data/one.row"), document).unwrap();
        root
    }

    for target in [65_536usize, 65_537] {
        let root = fixture(&target.to_string(), target);
        let definition = crate::source::test_support::document_folder("data", "data");
        let mut indexes = Vec::new();
        for workers in [1usize, 4] {
            let index = crate::lifecycle::Index::start_build(
                &root,
                vec![definition.clone()],
                RepositoryOptions {
                    query_workers: workers,
                    ..RepositoryOptions::default()
                },
                crate::source::test_support::fixture_registry(),
            )
            .and_then(|task| task.wait_complete())
            .unwrap();
            let view = index.view().unwrap();
            assert_eq!(
                view.segments(CollectionId(0)).unwrap()[0]
                    .native_payload()
                    .max_record_range_len(),
                target as u64
            );
            let result = view.search(&all_query()).unwrap();
            assert_eq!(result.count, Some(1));
            assert_eq!(result.hits.len(), 1);
            indexes.push((index, result));
        }
        assert_eq!(indexes[0].1.hits, indexes[1].1.hits);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            Some(cancelled),
        );
        let outcome = indexes[0]
            .0
            .view()
            .unwrap()
            .search_stream_scoped_with_context(
                &all_query(),
                ResultScope::All { offset: 0 },
                StreamOptions::default(),
                &context,
                &mut |_| Ok(StreamControl::Continue),
            )
            .unwrap();
        assert!(matches!(outcome, StreamOutcome::Cancelled(_)));
        for (index, _) in indexes {
            index.shutdown().unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    assert!(crate::query_execution::runner::record_ranges_allow_parallel_scan(65_536));
    assert!(!crate::query_execution::runner::record_ranges_allow_parallel_scan(65_537));
}

fn all_query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn adopted_segment_payloads_are_reclaimed_after_replacement_and_pinned_views() {
    let root = std::env::temp_dir().join(format!(
        "objekte-adopt-reclaim-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/item.row");
    std::fs::write(&path, br#"("id"="item","title"="old")"#).unwrap();
    let mut definition = crate::source::test_support::document_folder("data", "data");
    definition.identity =
        crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(crate::schema::FieldDefinition {
            path: vec!["id".into()],
            kind: crate::model::LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();

    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let weak = std::sync::Arc::downgrade(&repo.segments[0][0].index);
    let index = Index::from_repository(repo).unwrap();
    assert!(weak.upgrade().is_some());
    let view = index.view().unwrap();
    let handle = view.file_handle_for_path(&path).unwrap().unwrap();
    drop(view);
    std::fs::write(&path, br#"("id"="item","title"="new")"#).unwrap();
    index.update_file(handle, &path).unwrap();
    index.compact().unwrap();
    assert!(
        weak.upgrade().is_none(),
        "the adopted initial payload remained owned after replacement"
    );

    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let weak = std::sync::Arc::downgrade(&repo.segments[0][0].index);
    let index = Index::from_repository(repo).unwrap();
    let pinned = index.view().unwrap();
    let handle = pinned.file_handle_for_path(&path).unwrap().unwrap();
    std::fs::write(&path, br#"("id"="item","title"="newer")"#).unwrap();
    index.update_file(handle, &path).unwrap();
    index.compact().unwrap();
    assert!(weak.upgrade().is_some());
    let old = pinned
        .search(&Query {
            collections: vec!["data".into()],
            fts: None,
            filter: None,
            options: QueryOptions {
                sort: vec![],
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        })
        .unwrap();
    assert_eq!(old.count, Some(1));
    drop(pinned);
    assert!(weak.upgrade().is_none());

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
