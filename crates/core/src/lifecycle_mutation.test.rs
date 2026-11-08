use super::{FileMutationKind, Index, IndexError};
use crate::inventory::SourceRevision;
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;

fn test_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-mutation-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    root
}

fn index(root: &std::path::Path) -> Index {
    Index::build(
        root,
        vec![CollectionDefinition::folder(
            "items",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap()
}

#[test]
fn public_mutations_add_update_move_and_preserve_version_conflicts() {
    let root = test_root("path");
    let index = index(&root);
    let path = root.join("data/first.jsonl");
    std::fs::write(&path, b"{\"name\":\"one\"}\n").unwrap();
    let added = index.add_file(&path).unwrap();
    assert_eq!(added.kind, FileMutationKind::Added);
    assert_eq!(added.retired_records, 0);
    assert_eq!(added.indexed_records, 1);
    assert_eq!(added.affected_collections, ["items"]);
    assert_eq!(index.status().unwrap().live_records, 1);
    assert!(matches!(
        index.add_file(&path),
        Err(IndexError::DestinationExists)
    ));

    let same = index.update_file(added.file, &path).unwrap();
    assert_eq!(same.kind, FileMutationKind::Unchanged);
    assert_eq!(same.file, added.file);
    assert_eq!(same.revision, added.revision);

    std::fs::write(&path, b"{\"name\":\"two\"}\n{\"name\":\"three\"}\n").unwrap();
    let changed = index.update_file(added.file, &path).unwrap();
    assert_eq!(changed.kind, FileMutationKind::Replaced);
    assert_eq!(changed.retired_records, 1);
    assert_eq!(changed.indexed_records, 2);
    assert_eq!(changed.file.file_id, added.file.file_id);
    assert_ne!(changed.file.version, added.file.version);
    assert_eq!(index.status().unwrap().live_records, 2);
    assert!(matches!(
        index.update_file(added.file, &path),
        Err(IndexError::FileConflict { .. })
    ));

    let destination = root.join("data/moved.jsonl");
    std::fs::rename(&path, &destination).unwrap();
    let moved = index.move_file(changed.file, &destination).unwrap();
    assert_eq!(moved.kind, FileMutationKind::Moved);
    assert_eq!(moved.retired_records, 2);
    assert_eq!(moved.indexed_records, 2);
    assert_eq!(moved.file.file_id, changed.file.file_id);
    assert_ne!(moved.file.version, changed.file.version);
    assert!(index.view().unwrap().file_handle_for_path(&path).is_none());
    assert_eq!(
        index.view().unwrap().file_handle_for_path(&destination),
        Some(moved.file)
    );
    assert!(!path.exists());
    assert!(destination.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supplied_bytes_update_preserves_source_and_checks_deletion_conflict() {
    let root = test_root("bytes");
    let index = index(&root);
    let path = root.join("data/file.jsonl");
    std::fs::write(&path, b"{\"name\":\"old\"}\n").unwrap();
    let added = index.add_file(&path).unwrap();

    let bytes = b"{\"name\":\"first\"}\n{\"name\":\"second\"}\n";
    std::fs::write(&path, bytes).unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let updated = index
        .update_file_from_bytes(added.file, &path, bytes, revision)
        .unwrap();
    assert_eq!(updated.kind, FileMutationKind::Replaced);
    assert_eq!(updated.retired_records, 1);
    assert_eq!(updated.indexed_records, 2);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let same = index
        .update_file_from_bytes(updated.file, &path, bytes, revision)
        .unwrap();
    assert_eq!(same.kind, FileMutationKind::Unchanged);
    assert_eq!(same.revision, updated.revision);
    assert_eq!(same.file, updated.file);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    let wrong = SourceRevision {
        size: revision.size + 1,
        ..revision
    };
    assert!(matches!(
        index.update_file_from_bytes(updated.file, &path, bytes, wrong),
        Err(IndexError::SourceUnavailable(_))
    ));
    assert_eq!(index.status().unwrap().revision, updated.revision);

    std::fs::remove_file(&path).unwrap();
    index.delete_file(updated.file).unwrap();
    assert!(matches!(
        index.update_file_from_bytes(updated.file, &path, bytes, revision),
        Err(IndexError::FileConflict { .. })
    ));
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_rejects_destination_owned_by_another_file() {
    let root = test_root("collision");
    let index = index(&root);
    let first = root.join("data/one.jsonl");
    let second = root.join("data/two.jsonl");
    std::fs::write(&first, b"{\"name\":\"one\"}\n").unwrap();
    std::fs::write(&second, b"{\"name\":\"two\"}\n").unwrap();
    let one = index.add_file(&first).unwrap();
    index.add_file(&second).unwrap();
    let before = index.status().unwrap();
    assert!(matches!(
        index.move_file(one.file, &second),
        Err(IndexError::DestinationExists)
    ));
    assert_eq!(index.status().unwrap(), before);
    assert!(first.exists() && second.exists());
    std::fs::remove_dir_all(root).unwrap();
}
