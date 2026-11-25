use super::*;
use crate::model::CollectionId;
use crate::query::options::ReturnMode;
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};

fn all() -> Query {
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

fn literal(text: &str) -> Query {
    let mut query = all();
    query.fts = Some(FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: text.into(),
        pattern_parameter: None,
        fields: vec![],
    });
    query
}

#[test]
fn compacting_native_segments_preserves_text_postings() {
    let root = std::env::temp_dir().join(format!(
        "objekte-native-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let keep = root.join("data/keep.row");
    let remove = root.join("data/remove.row");
    std::fs::write(&keep, br#"("name"="bravo needle")"#).unwrap();
    std::fs::write(&remove, br#"("name"="other needle")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    assert_eq!(
        pinned.search(&literal("bravo needle")).unwrap().count,
        Some(1)
    );
    index
        .delete_file(pinned.file_handle_for_path(&remove).unwrap().unwrap())
        .unwrap();
    std::fs::remove_file(&remove).unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&literal("bravo needle"))
            .unwrap()
            .count,
        Some(1)
    );
    assert_eq!(
        pinned.search(&literal("bravo needle")).unwrap().count,
        Some(1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compaction_reuses_unaffected_native_segment() {
    let root = std::env::temp_dir().join(format!(
        "objekte-selective-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let first = root.join("data/first.row");
    std::fs::write(&first, br#"("name"="first")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let removed = root.join("data/removed.row");
    let kept = root.join("data/kept.row");
    std::fs::write(&removed, br#"("name"="removed")"#).unwrap();
    std::fs::write(&kept, br#"("name"="kept needle")"#).unwrap();
    index.add_file(&removed).unwrap();
    index.add_file(&kept).unwrap();
    let pinned = index.view().unwrap();
    let unchanged = pinned.0.segments[0].last().unwrap().clone();
    let unchanged_accounting = unchanged.index.accounting;
    assert!(unchanged_accounting.retained_bytes_estimated > 0);
    assert!(unchanged_accounting.source_bytes_estimated > 0);
    assert_eq!(
        pinned.search(&literal("kept needle")).unwrap().count,
        Some(1)
    );
    index
        .delete_file(pinned.file_handle_for_path(&removed).unwrap().unwrap())
        .unwrap();
    std::fs::remove_file(&removed).unwrap();
    index.compact().unwrap();
    let current = index.view().unwrap();
    assert!(
        current.0.segments[0]
            .iter()
            .any(|segment| Arc::ptr_eq(segment, &unchanged))
    );
    assert_eq!(
        unchanged.index.accounting.retained_bytes_estimated,
        unchanged_accounting.retained_bytes_estimated
    );
    assert_eq!(
        current.search(&literal("kept needle")).unwrap().count,
        Some(1)
    );
    assert_eq!(
        pinned.search(&literal("kept needle")).unwrap().count,
        Some(1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_compaction_can_resume_remaining_retired_shards() {
    let root = std::env::temp_dir().join(format!(
        "objekte-bounded-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let keep = root.join("data/keep.row");
    std::fs::write(&keep, br#"("name"="keep")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    for number in 0..2 {
        let path = root.join(format!("data/remove-{number}.row"));
        std::fs::write(&path, br#"("name"="remove")"#).unwrap();
        let handle = index.add_file(&path).unwrap().file;
        index.delete_file(handle).unwrap();
        std::fs::remove_file(&path).unwrap();
    }
    let old = index.view().unwrap();
    assert_eq!(slot_count(&old.0), 3);
    let budget = CompactionBudget {
        records: 1,
        input_bytes: u64::MAX,
        temporary_bytes: u64::MAX,
        elapsed: Duration::from_secs(60),
    };
    let (first, work) =
        compact_published_with_budget(&old.0, old.revision().next().unwrap(), || false, budget)
            .unwrap();
    assert_eq!(work.records, 1);
    assert_eq!(slot_count(&first), 2);
    let (second, work) =
        compact_published_with_budget(&first, first.revision.next().unwrap(), || false, budget)
            .unwrap();
    assert_eq!(work.records, 1);
    assert_eq!(slot_count(&second), 1);
    assert_eq!(old.search(&all()).unwrap().count, Some(1));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compact_reclaims_base_and_segment_tombstones_without_source_files() {
    let root = std::env::temp_dir().join(format!(
        "objekte-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.row");
    let b = root.join("data/b.row");
    std::fs::write(&a, br#"("name"="A")"#).unwrap();
    std::fs::write(&b, br#"("name"="B")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let initial = index.view().unwrap().revision();
    let no_op = index.compact().unwrap();
    assert_eq!(no_op.revision, initial);
    assert_eq!(no_op.retired_slots_removed, 0);
    let old = index.view().unwrap();
    let a_handle = old.file_handle_for_path(&a).unwrap().unwrap();
    let b_handle = old.file_handle_for_path(&b).unwrap().unwrap();
    index.delete_file(a_handle).unwrap();
    std::fs::write(&b, br#"("name"="B2")"#).unwrap();
    index.update_file(b_handle, &b).unwrap();
    std::fs::remove_file(&a).unwrap();
    std::fs::remove_file(&b).unwrap();
    let before = index.view().unwrap().search(&all()).unwrap();
    assert_eq!(before.count, Some(1));
    let revision_before = index.view().unwrap().revision();
    let checks = std::cell::Cell::new(0);
    let cancelled = index.compact_with_control(
        || {
            checks.set(checks.get() + 1);
            checks.get() >= 4
        },
        |_, _| {},
    );
    assert_eq!(cancelled.unwrap_err(), IndexError::Cancelled);
    assert_eq!(index.view().unwrap().revision(), revision_before);
    let report = index.compact().unwrap();
    assert_eq!(report.slots_before, 3);
    assert_eq!(report.slots_after, 1);
    assert_eq!(report.retired_slots_removed, 2);
    assert_eq!(report.remaining_retired_slots, 0);
    let after = index.view().unwrap().search(&all()).unwrap();
    assert_eq!(after.count, before.count);
    assert_eq!(after.hits.len(), before.hits.len());
    assert_eq!(after.hits[0].record_id, before.hits[0].record_id);
    assert_eq!(after.hits[0].file, before.hits[0].file);
    assert_eq!(after.hits[0].generation, report.revision.0);
    assert_eq!(old.search(&all()).unwrap().count, Some(2));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compacted_base_contributions_preserve_native_queries() {
    let root = std::env::temp_dir().join(format!(
        "objekte-compact-cache-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.row");
    let b = root.join("data/b.row");
    std::fs::write(&a, br#"("name"="A")"#).unwrap();
    std::fs::write(&b, br#"("name"="B")"#).unwrap();
    let definitions = vec![crate::source::test_support::document_folder("data", "data")];
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&a)
        .unwrap()
        .unwrap();
    index.delete_file(handle).unwrap();
    std::fs::remove_file(&a).unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    assert_eq!(index.view().unwrap().search(&all()).unwrap().count, Some(1));
    let scan = index.start_reconcile().unwrap().wait().unwrap();
    assert_eq!(scan.added + scan.updated + scan.removed, 0);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_compaction_preserves_pinned_reader_and_reports_completion() {
    let root = std::env::temp_dir().join(format!(
        "objekte-background-compact-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let file = root.join("data/a.row");
    std::fs::write(&file, br#"("name"="A")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let old = index.view().unwrap();
    index
        .delete_file(old.file_handle_for_path(&file).unwrap().unwrap())
        .unwrap();
    let task = index.start_compaction().unwrap();
    assert_eq!(
        task.progress().unwrap().operation,
        crate::lifecycle::maintenance::MaintenanceOperation::Compact
    );
    let crate::lifecycle::maintenance::MaintenanceReport::Compact(report) = task.wait().unwrap()
    else {
        panic!("wrong operation")
    };
    assert_eq!(report.retired_slots_removed, 1);
    assert!(task.is_complete().unwrap());
    assert_eq!(
        task.progress().unwrap().phase,
        crate::lifecycle::maintenance::MaintenancePhase::Complete
    );
    assert_eq!(old.search(&all()).unwrap().count, Some(1));
    assert_eq!(index.view().unwrap().search(&all()).unwrap().count, Some(0));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segment_admission_keeps_large_partially_retired_shard_when_small_pairs_fit() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-admission-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(
            root.join(format!("data/{name}.row")),
            format!("(\"name\"=\"{name}\")"),
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
    let a = root.join("data/a.row");
    let b = root.join("data/b.row");
    let initial = index.view().unwrap();
    index
        .delete_file(initial.file_handle_for_path(&a).unwrap().unwrap())
        .unwrap();
    index.compact().unwrap();
    let b_handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&b)
        .unwrap()
        .unwrap();
    std::fs::write(&b, br#"("name"="bb")"#).unwrap();
    index.update_file(b_handle, &b).unwrap();
    let dense = index.view().unwrap().segments(CollectionId(0)).unwrap()[0].clone();
    assert!(dense.active.len() < dense.index.records.len() as u64);
    for number in 0..70 {
        let path = root.join(format!("data/new-{number}.row"));
        std::fs::write(&path, format!("(\"name\"=\"new-{number}\")")).unwrap();
        index.add_file(&path).unwrap();
    }
    let current = index.view().unwrap();
    assert!(
        current
            .segments(CollectionId(0))
            .unwrap()
            .iter()
            .any(|segment| Arc::ptr_eq(segment, &dense))
    );
    assert_eq!(current.search(&all()).unwrap().count, Some(72));
    assert_eq!(current.search(&literal("new-69")).unwrap().count, Some(1));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

/// Run with `cargo test --release -p objekte-search-core --lib
/// lifecycle_compaction::tests::large_source_free_compaction -- --ignored --nocapture`.
/// Set `OBJEKTE_LARGE_COMPACT_RECORDS=1000001` for the million-record case.
#[test]
#[ignore = "manual scale and peak-memory validation"]
fn large_source_free_compaction_cancels_and_retries_a_concurrent_commit() {
    use std::io::Write;

    let count = std::env::var("OBJEKTE_LARGE_COMPACT_RECORDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(262_144);
    assert!(count >= 262_144);
    let root = std::env::temp_dir().join(format!(
        "objekte-large-compact-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let many = root.join("data/many.rows");
    let single = root.join("data/a-single.rows");
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&many).unwrap());
    for number in 0..count {
        writeln!(writer, "(\"name\"=\"item-{number}\")").unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    std::fs::write(&single, b"(\"name\"=\"retired\")\n").unwrap();
    let started = Instant::now();
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
    eprintln!(
        "large compaction build records={count} elapsed={:?}",
        started.elapsed()
    );
    let pinned = index.view().unwrap();
    assert_eq!(pinned.search(&all()).unwrap().count, Some(count + 1));
    index
        .delete_file(pinned.file_handle_for_path(&single).unwrap().unwrap())
        .unwrap();
    std::fs::remove_file(&many).unwrap();
    std::fs::remove_file(&single).unwrap();
    let revision = index.view().unwrap().revision();
    let checks = std::cell::Cell::new(0);
    assert_eq!(
        index
            .compact_with_control(
                || {
                    checks.set(checks.get() + 1);
                    checks.get() >= 5
                },
                |_, _| {}
            )
            .unwrap_err(),
        IndexError::Cancelled
    );
    assert_eq!(index.view().unwrap().revision(), revision);
    assert_eq!(
        index.view().unwrap().search(&all()).unwrap().count,
        Some(count)
    );
    let late = root.join("data/late.rows");
    let published_late = std::cell::Cell::new(false);
    let started = Instant::now();
    let report = index
        .compact_with_control(
            || false,
            |phase, _| {
                if phase == MaintenancePhase::Publishing && !published_late.get() {
                    std::fs::write(&late, b"(\"name\"=\"late\")\n").unwrap();
                    index.add_file(&late).unwrap();
                    published_late.set(true);
                }
            },
        )
        .unwrap();
    eprintln!(
        "large compaction records={count} elapsed={:?} attempts={} before={} after={} retired={}",
        started.elapsed(),
        report.attempts,
        report.slots_before,
        report.slots_after,
        report.retired_slots_removed
    );
    assert_eq!(report.attempts, 2);
    assert_eq!(report.retired_slots_removed, 1);
    assert_eq!(report.slots_after, (count + 1) as u64);
    assert_eq!(
        index.view().unwrap().search(&all()).unwrap().count,
        Some(count + 1)
    );
    assert_eq!(pinned.search(&all()).unwrap().count, Some(count + 1));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
