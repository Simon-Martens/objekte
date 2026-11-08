use std::collections::BTreeMap;

use super::{ScanChange, plan_scan, scan_sources};
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
