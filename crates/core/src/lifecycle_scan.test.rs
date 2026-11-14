use std::collections::BTreeMap;

use super::{ScanChange, scan_sources_batched};
use crate::discovery::Discovery;
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::{SourceFormat, SourceRegistry};

#[test]
fn metadata_diff_preserves_failed_collection_and_detects_other_changes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-lifecycle-scan-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::write(root.join("a/one.json"), b"{\"x\":1}").unwrap();
    std::fs::write(root.join("b/two.json"), b"{\"x\":2}").unwrap();
    let root = root.canonicalize().unwrap();
    let definitions = vec![
        CollectionDefinition::folder("a", "a", SourceFormat::Json),
        CollectionDefinition::folder("b", "b", SourceFormat::Json),
    ];
    let sources = SourceRegistry::builtin();
    let mut current = BTreeMap::new();
    let baseline = scan_sources_batched(
        &root,
        &definitions,
        &sources,
        |_| false,
        |_| None,
        || false,
        1,
        |batch| {
            for change in batch {
                let ScanChange::Add(file) = change else {
                    panic!("expected add")
                };
                let id = FileId(current.len() as u32);
                current.insert(file.path.clone(), (id, file));
            }
        },
    );
    assert_eq!(baseline.examined, 2);
    assert!(baseline.issues.is_empty());

    std::fs::write(root.join("a/one.json"), b"{\"x\":123}").unwrap();
    std::fs::write(root.join("a/new.json"), b"{\"x\":3}").unwrap();
    std::fs::remove_dir_all(root.join("b")).unwrap();
    let mut changes = Vec::new();
    let scan = scan_sources_batched(
        &root,
        &definitions,
        &sources,
        |_| false,
        |path| current.get(path).cloned(),
        || false,
        1,
        |batch| changes.extend(batch),
    );
    assert_eq!(scan.complete_collections.len(), 1);
    assert_eq!(scan.issues.len(), 1);
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|change| matches!(
        change,
        ScanChange::Update { previous, observed }
            if previous.path == std::path::Path::new("a/one.json")
                && observed.path == previous.path
    )));
    assert!(changes.iter().any(|change| matches!(
        change,
        ScanChange::Add(observed)
            if observed.path == std::path::Path::new("a/new.json")
    )));
    let missing = current.get(std::path::Path::new("b/two.json")).unwrap();
    assert!(!scan.seen_existing.contains(missing.0.0));
    assert!(!missing.1.collections.is_subset(&scan.complete_collections));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn overlapping_collections_emit_one_physical_observation_in_small_batches() {
    let root = std::env::temp_dir().join(format!(
        "objekte-overlap-scan-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let root = root.canonicalize().unwrap();
    for name in ["one.json", "two.json", "three.json"] {
        std::fs::write(root.join("data").join(name), b"{\"name\":\"a\"}").unwrap();
    }
    let definitions = vec![
        CollectionDefinition::folder("first", "data", SourceFormat::Json),
        CollectionDefinition::folder("second", "data", SourceFormat::Json),
    ];
    let mut batches = Vec::new();
    let scan = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |_| None,
        || false,
        1,
        |changes| batches.push(changes),
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    assert_eq!(scan.complete_collections.len(), 2);
    assert_eq!(scan.examined, 3);
    assert_eq!(scan.planned_changes, 3);
    assert_eq!(batches.len(), 3);
    assert!(batches.iter().all(|batch| batch.len() == 1));
    for batch in batches {
        let ScanChange::Add(file) = &batch[0] else {
            panic!("expected add")
        };
        assert_eq!(file.collections, [CollectionId(0), CollectionId(1)].into());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nested_source_roots_preserve_all_memberships_without_duplicate_observation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-nested-overlap-scan-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/top.json"), b"{}").unwrap();
    std::fs::write(root.join("data/nested/inner.json"), b"{}").unwrap();
    let root = root.canonicalize().unwrap();
    let mut outer = CollectionDefinition::folder("outer", "data", SourceFormat::Json);
    outer.discovery.recursive = true;
    let inner = CollectionDefinition::folder("inner", "data/nested", SourceFormat::Json);
    let mut changes = Vec::new();
    let scan = scan_sources_batched(
        &root,
        &[outer, inner],
        &SourceRegistry::builtin(),
        |_| false,
        |_| None,
        || false,
        2,
        |batch| changes.extend(batch),
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    assert_eq!(scan.examined, 2);
    assert_eq!(scan.metadata_observations, 2);
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|change| matches!(change,
        ScanChange::Add(file) if file.path == std::path::Path::new("data/nested/inner.json")
            && file.collections == [CollectionId(0), CollectionId(1)].into()
    )));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_scan_marks_unchanged_files_and_reports_only_a_changed_file() {
    let root = std::env::temp_dir().join(format!(
        "objekte-incremental-batched-scan-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    std::fs::write(root.join("data/b.json"), b"{}").unwrap();
    let root = root.canonicalize().unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "one",
        "data",
        SourceFormat::Json,
    )];
    let mut prior = BTreeMap::new();
    let first = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |_| None,
        || false,
        1,
        |batch| {
            for change in batch {
                let ScanChange::Add(file) = change else {
                    panic!("expected add")
                };
                let id = FileId(prior.len() as u32);
                prior.insert(file.path.clone(), (id, file));
            }
        },
    );
    assert_eq!(first.planned_changes, 2);
    let mut changes = Vec::new();
    let second = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |path| prior.get(path).cloned(),
        || false,
        1,
        |batch| changes.extend(batch),
    );
    assert!(changes.is_empty());
    assert_eq!(second.unchanged, 2);
    assert_eq!(second.seen_existing.len(), 2);
    std::fs::write(root.join("data/b.json"), b"{\"changed\":true}").unwrap();
    let third = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |path| prior.get(path).cloned(),
        || false,
        1,
        |batch| changes.extend(batch),
    );
    assert_eq!(third.unchanged, 1);
    assert_eq!(third.planned_changes, 1);
    assert!(
        matches!(&changes[..], [ScanChange::Update { previous, observed }]
        if previous.path == std::path::Path::new("data/b.json")
            && observed.revision.size > previous.revision.size)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_overlap_scope_preserves_existing_file_without_partial_replacement() {
    let root = std::env::temp_dir().join(format!(
        "objekte-failed-overlap-scan-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("data/one.json"), b"{\"x\":1}").unwrap();
    let mut definitions = vec![
        CollectionDefinition::folder("first", "data", SourceFormat::Json),
        CollectionDefinition::folder("second", "data", SourceFormat::Json),
    ];
    let mut original = Vec::new();
    let baseline = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |_| None,
        || false,
        1,
        |batch| original.extend(batch),
    );
    assert!(baseline.issues.is_empty());
    let [ScanChange::Add(previous)] = &original[..] else {
        panic!("expected one add")
    };
    let previous = previous.clone();
    std::fs::write(root.join("data/one.json"), b"{\"x\":1234}").unwrap();
    definitions[1].discovery = Discovery {
        include: vec!["[".into()],
        ..Discovery::default()
    };
    let mut batches = Vec::new();
    let scan = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |_| Some((FileId(0), previous.clone())),
        || false,
        1,
        |changes| batches.push(changes),
    );
    assert_eq!(scan.complete_collections.len(), 1);
    assert_eq!(scan.issues.len(), 1);
    assert!(scan.seen_existing.contains(0));
    assert_eq!(scan.skipped, 1);
    assert!(batches.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_exclusion_precedes_seen_tracking_and_scan_cancels_between_batches() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let root = std::env::temp_dir().join(format!(
        "objekte-batched-scan-cancel-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/cache.json"), b"{}").unwrap();
    for n in 0..4 {
        std::fs::write(root.join(format!("data/item-{n}.json")), b"{}").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "one",
        "data",
        SourceFormat::Json,
    )];
    let cancelled = AtomicBool::new(false);
    let mut batches = Vec::new();
    let scan = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |path| path == std::path::Path::new("data/cache.json"),
        |path| {
            (path == std::path::Path::new("data/cache.json")).then_some((
                FileId(9),
                super::FileStamp {
                    path: path.to_owned(),
                    revision: crate::inventory::SourceRevision {
                        modified_ns: 0,
                        size: 0,
                    },
                    created_ns: None,
                    collections: [CollectionId(0)].into(),
                    retry_unreadable: false,
                },
            ))
        },
        || cancelled.load(Ordering::Relaxed),
        1,
        |changes| {
            batches.push(changes);
            cancelled.store(true, Ordering::Relaxed);
        },
    );
    assert!(scan.cancelled);
    assert!(!scan.seen_existing.contains(9));
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "creates 300,000 disposable files; run with OBJEKTE_SCAN_SCALE_ROOT under target/"]
fn scans_300000_files_with_two_overlapping_150000_file_collections() {
    use std::time::Instant;

    let root = std::path::PathBuf::from(
        std::env::var_os("OBJEKTE_SCAN_SCALE_ROOT")
            .expect("set OBJEKTE_SCAN_SCALE_ROOT to a disposable target/ directory"),
    );
    assert!(root.to_string_lossy().contains("target/"));
    if !root.join("ready").exists() {
        for kind in ["shared", "unique"] {
            std::fs::create_dir_all(root.join("data").join(kind)).unwrap();
            for number in 0..150_000 {
                std::fs::write(
                    root.join("data")
                        .join(kind)
                        .join(format!("{number:06}.json")),
                    b"{}",
                )
                .unwrap();
            }
        }
        std::fs::write(root.join("ready"), b"300000").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let definitions = [
        ("first", "shared/*.json"),
        ("second", "shared/*.json"),
        ("third", "unique/*.json"),
    ]
    .into_iter()
    .map(|(name, pattern)| {
        let mut definition = CollectionDefinition::folder(name, "data", SourceFormat::Json);
        definition.discovery = Discovery {
            recursive: true,
            include: vec![pattern.into()],
            ..Discovery::default()
        };
        definition
    })
    .collect::<Vec<_>>();
    let started = Instant::now();
    let mut emitted = 0;
    let mut shared = 0;
    let mut max_batch = 0;
    let scan = scan_sources_batched(
        &root,
        &definitions,
        &SourceRegistry::builtin(),
        |_| false,
        |_| None,
        || false,
        256,
        |batch| {
            max_batch = max_batch.max(batch.len());
            emitted += batch.len();
            shared += batch
                .iter()
                .filter(|change| match change {
                    ScanChange::Add(file) => file.collections.len() == 2,
                    _ => false,
                })
                .count();
        },
    );
    eprintln!(
        "scan_scale files={} metadata={} batches_max={} elapsed_ms={}",
        scan.examined,
        scan.metadata_observations,
        max_batch,
        started.elapsed().as_millis()
    );
    assert!(scan.issues.is_empty(), "{:?}", scan.issues);
    assert_eq!(scan.complete_collections.len(), 3);
    assert_eq!(scan.examined, 300_000);
    assert_eq!(scan.metadata_observations, 300_000);
    assert_eq!(emitted, 300_000);
    assert_eq!(shared, 150_000);
    assert!(max_batch <= 256);
}

#[test]
#[ignore = "uses the disposable 300,000-file fixture from the scanner scale test"]
fn reconciles_300000_files_and_updates_a_shared_file() {
    use crate::lifecycle::Index;
    use crate::repository::{RepositoryOptions};
    use std::time::Instant;

    let root = std::path::PathBuf::from(
        std::env::var_os("OBJEKTE_SCAN_SCALE_ROOT")
            .expect("set OBJEKTE_SCAN_SCALE_ROOT to the disposable target/ scale fixture"),
    );
    assert!(root.to_string_lossy().contains("target/"));
    assert_eq!(std::fs::read(root.join("ready")).unwrap(), b"300000");
    let root = root.canonicalize().unwrap();
    let definitions = [
        ("first", "shared/*.json"),
        ("second", "shared/*.json"),
        ("third", "unique/*.json"),
    ]
    .into_iter()
    .map(|(name, pattern)| {
        let mut definition = CollectionDefinition::folder(name, "data", SourceFormat::Json);
        definition.discovery = Discovery {
            recursive: true,
            include: vec![pattern.into()],
            ..Discovery::default()
        };
        definition
    })
    .collect::<Vec<_>>();
    let started = Instant::now();
    let index = Index::build(
        &root,
        definitions,
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    let build_ms = started.elapsed().as_millis();
    let initial = index.status().unwrap();
    assert_eq!(initial.indexed_files, 300_000);
    assert_eq!(initial.live_records, 450_000);

    let started = Instant::now();
    let unchanged = index.reconcile_pass(|| false);
    let unchanged_ms = started.elapsed().as_millis();
    assert!(unchanged.issues.is_empty(), "{:?}", unchanged.issues);
    assert_eq!(unchanged.examined, 300_000);
    assert_eq!(unchanged.unchanged, 300_000);
    assert_eq!(unchanged.indexed_bytes, 0);
    assert_eq!(
        (unchanged.added, unchanged.updated, unchanged.removed),
        (0, 0, 0)
    );
    assert_eq!(index.status().unwrap().revision, initial.revision);

    let changed_path = root.join("data/shared/000000.json");
    let original = std::fs::read(&changed_path).unwrap();
    std::fs::write(&changed_path, b"{\"changed\":true}").unwrap();
    let started = Instant::now();
    let updated = index.reconcile_pass(|| false);
    let updated_ms = started.elapsed().as_millis();
    std::fs::write(&changed_path, original).unwrap();
    assert!(updated.issues.is_empty(), "{:?}", updated.issues);
    assert_eq!(updated.examined, 300_000);
    assert_eq!((updated.added, updated.updated, updated.removed), (0, 1, 0));
    assert_eq!(updated.indexed_bytes, b"{\"changed\":true}".len() as u64);
    let view = index.view().unwrap();
    let file_id = view
        .0
        .catalog
        .id_for(std::path::Path::new("data/shared/000000.json"))
        .unwrap();
    let file = view.0.catalog.entry(file_id).unwrap();
    assert_eq!(file.collections.len(), 2);
    eprintln!(
        "reconcile_scale build_ms={build_ms} unchanged_ms={unchanged_ms} updated_ms={updated_ms} files={} records={} shared_memberships={} unchanged_indexed_bytes={} updated_indexed_bytes={}",
        initial.indexed_files,
        initial.live_records,
        file.collections.len(),
        unchanged.indexed_bytes,
        updated.indexed_bytes,
    );
}
