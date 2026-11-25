use super::*;
use crate::repository::configuration::RepositoryOptions;
use crate::source::SourceFormat;
use crate::source::registry::SourceRegistry;
use std::fs;

fn test_root(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "objekte-config-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[test]
fn source_preparation_preserves_live_identity_and_generated_exclusions() {
    let root = test_root("source-prepare");
    fs::create_dir_all(root.join("data")).unwrap();
    let definition = CollectionDefinition::folder("files", "", SourceFormat::JsonDecodedScalars);
    let first = prepare_source_definitions(&root, std::slice::from_ref(&definition)).unwrap();
    assert_eq!(
        first[0].discovery.exclude_directories,
        vec![
            PathBuf::from(".objekte/.indexes"),
            PathBuf::from(".objekte/indexes")
        ]
    );
    let second = prepare_source_definitions(&root, &first).unwrap();
    assert_eq!(second[0].discovery, first[0].discovery);
    assert_eq!(second[0].source_path, first[0].source_path);
    let hash = crate::repository::configuration::identity::configuration_hash(
        &first,
        RepositoryOptions::default(),
        &SourceRegistry::builtin(),
    )
    .unwrap();
    let actual = hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        actual,
        "d025ae2e90ad25b5c5fb46e60f4247bbce8bf9338613cfd17f43185db7016a93"
    );
    fs::create_dir_all(root.join(".objekte/.indexes")).unwrap();
    let reserved =
        CollectionDefinition::folder("bad", ".objekte/.indexes", SourceFormat::JsonDecodedScalars);
    assert!(
        prepare_source_definitions(&root, &[reserved])
            .unwrap_err()
            .contains("generated index namespace")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_selection_rejects_generated_namespaces_and_aliases() {
    let root = test_root("generated-source");
    fs::create_dir_all(root.join("data/nested/objekte-indexes")).unwrap();
    fs::create_dir_all(root.join(".objekte/indexes")).unwrap();
    fs::create_dir_all(root.join("cache/objekte-indexes")).unwrap();
    for source in [
        ".objekte",
        ".objekte/indexes",
        ".objekte/.indexes",
        "data/nested/objekte-indexes",
        "cache/objekte-indexes",
    ] {
        let definition =
            CollectionDefinition::folder("files", source, SourceFormat::JsonDecodedScalars);
        assert!(
            prepare_source_definitions(&root, &[definition])
                .unwrap_err()
                .contains("generated index namespace"),
            "{source}"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join(".objekte/indexes"), root.join("generated-alias"))
            .unwrap();
        let definition = CollectionDefinition::folder(
            "files",
            "generated-alias",
            SourceFormat::JsonDecodedScalars,
        );
        assert!(
            prepare_source_definitions(&root, &[definition])
                .unwrap_err()
                .contains("generated index namespace")
        );
    }
    let legitimate =
        CollectionDefinition::folder("files", "data", SourceFormat::JsonDecodedScalars);
    let prepared = prepare_source_definitions(&root, &[legitimate]).unwrap();
    assert_eq!(prepared[0].source_path, PathBuf::from("data"));
    fs::remove_dir_all(root).unwrap();
}
