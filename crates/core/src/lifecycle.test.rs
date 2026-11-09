use std::sync::Arc;

use super::{DeleteStrategy, Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::{FieldPath, FileId, LogicalType, RecordSlot};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;
use crate::streaming::{
    CancellationToken, ExecutionContext, ExecutionLimits, ResultScope, StreamControl, StreamEvent,
    StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;
use crate::workload::{SortExpression, SortField};

fn fixture_repository(root: &std::path::Path) -> Repository {
    Repository::open_with_options(
        root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap()
}

/// Keep tests that directly manipulate base slots on the legacy static
/// representation; normal `Index::build` now converts its base to partitions.
fn static_index(repository: Repository) -> Index {
    let published = PublishedIndex::from_repository(
        crate::repository::next_repository_instance(),
        IndexRevision::INITIAL,
        Arc::new(repository),
    )
    .unwrap();
    Index::from_published(published)
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
    use super::{ReconciliationJob, ReconciliationProgress, ReconciliationState};
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
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let old = index.view().unwrap();
    let b = root.join("data/b.json");
    let report = index
        .rebuild_with_before_publish(|attempt| {
            if attempt == 1 {
                std::fs::write(&b, br#"{"name":"B"}"#).unwrap();
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
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
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
    let super::MaintenanceReport::Rebuild(report) = task.wait().unwrap() else {
        panic!("wrong operation")
    };
    assert_eq!(report.revision, initial.next().unwrap());
    assert_eq!(
        task.progress().unwrap().phase,
        super::MaintenancePhase::Complete
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_direct_lookup_requests_reconciliation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-lookup-reconcile-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/a.json");
    std::fs::write(&path, br#"{"name":"A"}"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let view = index.view().unwrap();
    let id = view.search(&all_records()).unwrap().hits[0]
        .record_id
        .clone();
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        view.get_record_available("data", &id, ReturnMode::Bytes)
            .unwrap(),
        crate::run::DirectLookupResult::SourceUnavailable(_)
    ));
    assert_ne!(
        index.status().unwrap().reconciliation,
        super::ReconciliationState::NotStarted
    );
    index.shutdown().unwrap();
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
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
    let index = static_index(fixture_repository(&root));
    let clone = index.clone();
    let old = index.view().unwrap();
    let first_file = old.file_handle(FileId(0)).unwrap();
    assert_eq!(old.revision(), IndexRevision::INITIAL);
    assert_eq!(index.status().unwrap().revision, IndexRevision::INITIAL);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));

    std::fs::write(root.join("data/b.json"), b"{\"name\":\"B\"}").unwrap();
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
    assert_eq!(current.file_version(FileId(0)).unwrap().0, new_revision.0);
    assert_eq!(old.file_handle(FileId(0)).unwrap(), first_file);
    assert_eq!(old.search(&all_records()).unwrap().count, Some(1));
    assert_eq!(
        old.active_slots(crate::model::CollectionId(0))
            .unwrap()
            .len(),
        1
    );
    assert!(old.is_active(crate::model::CollectionId(0), crate::model::RecordSlot(0)));

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
    std::fs::write(root.join("data/a.json"), b"{\"name\":\"A\"}").unwrap();
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
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let index = Index::from_repository(fixture_repository(&root)).unwrap();
    let next_path = root.join("data/b.json");
    std::fs::write(&next_path, br#"{"name":"B"}"#).unwrap();
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
fn deleting_shared_multi_record_file_retires_every_collection_atomically() {
    let root = std::env::temp_dir().join(format!(
        "objekte-delete-file-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/shared.jsonl");
    std::fs::write(&source, b"{\"name\":\"A\"}\n{\"name\":\"B\"}\n").unwrap();
    let index = Index::build(
        &root,
        vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "data", SourceFormat::JsonLines),
        ],
        RepositoryOptions::default(),
    )
    .unwrap();
    let old = index.view().unwrap();
    let handle = old.file_handle(FileId(0)).unwrap();
    assert_eq!(old.file_handle_for_path(&source), Some(handle));
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
    assert_eq!(result.revision, IndexRevision(2));
    assert_eq!(result.retired_records, 4);
    assert_eq!(result.affected_collections, vec!["left", "right"]);
    assert_eq!(result.strategy, DeleteStrategy::Tombstone);
    assert_eq!(old.search(&query).unwrap().count, Some(4));
    let current = index.view().unwrap();
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
    assert!(current.file_handle(FileId(0)).is_none());
    assert!(current.file_handle_for_path(&source).is_none());
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
    std::fs::write(root.join("data/a.jsonl"), b"{\"x\":1}\n").unwrap();
    let index = Index::build(
        &root,
        vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "data", SourceFormat::JsonLines),
        ],
        RepositoryOptions::default(),
    )
    .unwrap();
    let before_view = index.view().unwrap();
    let prior_segments = before_view.0.segments[0].len();
    let next_ordinal = before_view.0.next_segment_ordinal;
    let before = index.status().unwrap();
    assert!(matches!(
        index.prepare_add_file(std::path::Path::new("data/a.jsonl")),
        Err(IndexError::DestinationExists)
    ));
    let added = root.join("data/b.jsonl");
    std::fs::write(&added, b"{\"x\":2}\n{\"x\":3}\n").unwrap();
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
    assert_eq!(staged.status().indexed_files, 2);
    assert_eq!(staged.status().live_records, 6);
    assert_eq!(staged.segments[0].len(), prior_segments + 1);
    assert_eq!(staged.segments[1].len(), prior_segments + 1);
    assert_eq!(staged.segments[0].last().unwrap().ordinal, next_ordinal);
    assert_eq!(staged.segments[1].last().unwrap().ordinal, next_ordinal + 1);
    assert_eq!(
        staged.catalog.id_for(std::path::Path::new("data/b.jsonl")),
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
    let old_path = root.join("data/old.jsonl");
    std::fs::write(&old_path, b"{\"name\":\"a\"}\n{\"name\":\"b\"}\n").unwrap();
    let index = Index::build(
        &root,
        vec![
            CollectionDefinition::folder("left", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("right", "data", SourceFormat::JsonLines),
        ],
        RepositoryOptions::default(),
    )
    .unwrap();
    let prior_segments = index.view().unwrap().0.segments[0].len();
    let before = index.status().unwrap();
    let handle = index.view().unwrap().file_handle(FileId(0)).unwrap();
    let new_path = root.join("data/new.jsonl");
    std::fs::write(&new_path, b"{\"name\":\"replacement\"}\n").unwrap();
    let prepared = index.prepare_replace_file(handle, &new_path).unwrap();
    let staged = index.stage_prepared_replace(prepared).unwrap();
    assert_eq!(staged.revision, before.revision.next().unwrap());
    assert_eq!(staged.status().indexed_files, 1);
    assert_eq!(staged.status().live_records, 2);
    assert_eq!(staged.active[0].len(), 0);
    assert_eq!(staged.active[1].len(), 0);
    assert_eq!(staged.segments[0].len(), prior_segments + 1);
    assert_eq!(staged.segments[1].len(), prior_segments + 1);
    assert_eq!(
        staged
            .catalog
            .id_for(std::path::Path::new("data/old.jsonl")),
        None
    );
    assert_eq!(
        staged
            .catalog
            .id_for(std::path::Path::new("data/new.jsonl")),
        Some(FileId(0))
    );
    assert_eq!(
        staged.file_versions.get(FileId(0)).unwrap().0,
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
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.file_path.ends_with("new.jsonl"))
    );
    for hit in &result.hits {
        assert_eq!(
            current
                .get_record(&hit.collection, &hit.record_id, ReturnMode::Locator)
                .unwrap()
                .unwrap()
                .file_path,
            hit.file_path
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
    let original = root.join("data/a.jsonl");
    std::fs::write(&original, b"{\"name\":\"a\"}\n").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let initial = index.status().unwrap();
    let unchanged = index.reconcile_pass(|| false);
    assert_eq!(unchanged.examined, 1);
    assert_eq!(unchanged.unchanged, 1);
    assert!(unchanged.issues.is_empty());
    assert_eq!(index.status().unwrap(), initial);

    let added = root.join("data/b.jsonl");
    std::fs::write(&added, b"{\"name\":\"b\"}\n").unwrap();
    let report = index.reconcile_pass(|| false);
    assert_eq!((report.added, report.updated, report.removed), (1, 0, 0));
    assert_eq!(
        report.indexed_bytes,
        std::fs::metadata(&added).unwrap().len()
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(index.status().unwrap().indexed_files, 2);

    std::fs::write(&original, b"{\"name\":\"replacement\"}\n").unwrap();
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
    std::fs::write(root.join("data/a.jsonl"), b"{\"name\":\"a\"}\n").unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let before = index.status().unwrap();
    let task = index.start_reconcile().unwrap();
    let report = task.wait().unwrap();
    assert_eq!(report.unchanged, 1);
    assert!(report.issues.is_empty());
    assert!(task.is_complete().unwrap());
    assert_eq!(
        task.progress().unwrap(),
        super::ReconciliationProgress::from(&report)
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
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    for number in 0..70 {
        let relative = format!("data/file-{number}.jsonl");
        std::fs::write(root.join(&relative), format!("{{\"number\":{number}}}\n")).unwrap();
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
    assert!(index.view().unwrap().0.segments[0].len() <= 64);

    let relative = std::path::Path::new("data/file-0.jsonl");
    for number in 0..70 {
        std::fs::write(root.join(relative), format!("{{\"updated\":{number}}}\n")).unwrap();
        let handle = index
            .view()
            .unwrap()
            .file_handle_for_path(relative)
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
    assert!(index.view().unwrap().0.segments[0].len() <= 64);
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
        root.join("data/shared.jsonl"),
        b"{\"sequence\":1,\"kind\":\"yes\",\"body\":\"needle\"}\n{\"sequence\":2,\"kind\":\"yes\",\"body\":\"needle\"}\n{\"sequence\":3,\"kind\":\"yes\",\"body\":\"needle\"}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("data/other.jsonl"),
        b"{\"sequence\":4,\"kind\":\"yes\",\"body\":\"needle\"}\n{\"sequence\":5,\"kind\":\"yes\",\"body\":\"needle\"}\n{\"sequence\":6,\"kind\":\"yes\",\"body\":\"needle\"}\n",
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
    let repository = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![FieldPath::parse("sequence").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
    )
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
    next.active[0] = old.0.active[0].retired(&retired).unwrap();
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
            crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
        ("targets/a.json", r#"{"id":"A","title":"Alpha"}"#),
        ("targets/b.json", r#"{"id":"B","title":"Beta"}"#),
        (
            "sources/s1.json",
            r#"{"id":"s1","entry":"A","title":"one"}"#,
        ),
        (
            "sources/s2.json",
            r#"{"id":"s2","entry":"B","title":"two"}"#,
        ),
        (
            "sources/s3.json",
            r#"{"id":"s3","entry":"A","title":"three"}"#,
        ),
        (
            "sources/s4.json",
            r#"{"id":"s4","entry":"B","title":"four"}"#,
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
        let mut definition = CollectionDefinition::folder(name, name, SourceFormat::Json);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        definition.schema = schema;
        definitions.push(definition);
    }
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let index = static_index(repository);
    let old = index.view().unwrap();
    let target_a = old.repository().collections()[0].id_lookup["A"];
    let source_s2 = old.repository().collections()[1].id_lookup["s2"];
    let mut next = PublishedIndex::from_repository(
        index.instance_id(),
        old.revision().next().unwrap(),
        Arc::clone(&old.0.repository),
    )
    .unwrap();
    next.active[0] = next.active[0].retired(&[target_a]).unwrap();
    next.active[1] = next.active[1].retired(&[source_s2]).unwrap();
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
