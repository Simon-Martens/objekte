use std::collections::BTreeMap;

use super::{ScanChange, plan_scan, scan_sources, scan_sources_batched};
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
    let definitions = vec![
        CollectionDefinition::folder("a", "a", SourceFormat::Json),
        CollectionDefinition::folder("b", "b", SourceFormat::Json),
    ];
    let sources = SourceRegistry::builtin();
    let baseline = scan_sources(&root, &definitions, &sources, || false);
    assert_eq!(baseline.files.len(), 2);
    assert!(baseline.issues.is_empty());
    let current: BTreeMap<_, _> = baseline.files;

    std::fs::write(root.join("a/one.json"), b"{\"x\":123}").unwrap();
    std::fs::write(root.join("a/new.json"), b"{\"x\":3}").unwrap();
    std::fs::remove_dir_all(root.join("b")).unwrap();
    let scan = scan_sources(&root, &definitions, &sources, || false);
    assert_eq!(scan.complete_collections.len(), 1);
    assert_eq!(scan.issues.len(), 1);
    let plan = plan_scan(&current, &scan);
    assert_eq!(plan.skipped, 1);
    assert_eq!(plan.changes.len(), 2);
    assert!(plan.changes.iter().any(|change| matches!(
        change,
        ScanChange::Update { previous, observed }
            if previous.path == std::path::Path::new("a/one.json")
                && observed.path == previous.path
    )));
    assert!(plan.changes.iter().any(|change| matches!(
        change,
        ScanChange::Add(observed)
            if observed.path == std::path::Path::new("a/new.json")
    )));
    assert!(
        !plan
            .changes
            .iter()
            .any(|change| matches!(change, ScanChange::Remove(_)))
    );
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
    let original = scan_sources(&root, &definitions, &SourceRegistry::builtin(), || false);
    let previous = original
        .files
        .get(std::path::Path::new("data/one.json"))
        .unwrap()
        .clone();
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
