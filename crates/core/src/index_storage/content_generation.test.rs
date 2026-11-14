use std::path::Path;

use crate::lifecycle::Index;
use crate::model::{FieldPath, FileId, LogicalType};
use crate::reference::FieldDefinition;
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::source::{SourceFormat, SourceRegistry};

use super::store::SaveOptions;

fn definition() -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
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
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["text".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    definition.sort_fields = vec![FieldPath::parse("text").unwrap()];
    definition
}

#[test]
fn content_generations_advance_for_replacements_and_survive_moves_and_reload() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    let first = root.path().join("data/a.json");
    std::fs::write(&first, br#"{"id":"a","text":"Alpha"}"#).unwrap();
    let definition = definition();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let view = index.view().unwrap();
    let initial = view.file_entry(FileId(0)).unwrap().content_generation;
    assert_eq!(initial, 1);
    let first_handle = view.file_handle(FileId(0)).unwrap();
    drop(view);

    std::fs::write(&first, br#"{"id":"a","text":"Replaced"}"#).unwrap();
    let replacement = index.update_file(first_handle, &first).unwrap();
    assert!(replacement.file.version.0 > first_handle.version.0);
    let view = index.view().unwrap();
    let after_replace = view.file_entry(FileId(0)).unwrap().content_generation;
    assert!(after_replace > initial);
    drop(view);

    let second = root.path().join("data/b.json");
    std::fs::rename(&first, &second).unwrap();
    let moved = index.move_file(replacement.file, &second).unwrap();
    assert!(moved.file.version.0 > replacement.file.version.0);
    let view = index.view().unwrap();
    let after_move = view.file_entry(FileId(0)).unwrap().content_generation;
    assert_eq!(after_move, after_replace);
    assert_eq!(
        view.file_entry(FileId(0)).unwrap().relative_path.as_path(),
        Path::new("data/b.json")
    );
    drop(view);

    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let view = loaded.view().unwrap();
    assert_eq!(
        view.file_entry(FileId(0)).unwrap().content_generation,
        after_move
    );
    assert_eq!(
        view.file_handle(FileId(0)).unwrap().version,
        moved.file.version
    );
    assert_ne!(after_move, moved.file.version.0);

    let loaded_handle = view.file_handle(FileId(0)).unwrap();
    drop(view);
    let deleted = loaded.delete_file(loaded_handle).unwrap();
    assert_eq!(deleted.file, FileId(0));
    assert!(loaded.view().unwrap().file_entry(FileId(0)).is_none());

    let deleted_store = tempfile::tempdir().unwrap();
    loaded
        .save_to(deleted_store.path(), SaveOptions::default())
        .unwrap();
    let opened = super::store::open_store(deleted_store.path()).unwrap();
    let catalogue = super::catalogue::CatalogueView::open(opened.catalogue_backing()).unwrap();
    let row = catalogue.file(0).unwrap().unwrap();
    assert!(row.tombstone);
    assert_eq!(row.content_generation, after_move);
    assert!(row.file_version > moved.file.version.0 - 1);
    drop(catalogue);
    drop(opened);
    let reloaded = Index::load_from(
        deleted_store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert!(reloaded.view().unwrap().file_entry(FileId(0)).is_none());
}
