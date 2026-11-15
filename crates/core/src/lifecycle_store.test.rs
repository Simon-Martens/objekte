use super::*;
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions};
use crate::reference::FieldDefinition;
use crate::repository::CollectionDefinition;
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-store-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    root
}

fn definition() -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    for (path, kind) in [("name", LogicalType::String), ("n", LogicalType::Number)] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
}

fn seed(root: &std::path::Path) {
    for number in 0..20usize {
        std::fs::write(
            root.join(format!("data/{number:03}.jsonl")),
            format!("{{\"name\":\"name-{number}\",\"n\":{number}}}\n"),
        )
        .unwrap();
    }
}

fn count(index: &Index) -> usize {
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 0,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    index
        .view()
        .unwrap()
        .search(&query)
        .unwrap()
        .count
        .unwrap_or(0)
}

fn options(path: &std::path::Path) -> StoreOptions {
    StoreOptions {
        path: Some(path.to_path_buf()),
        durable: false,
    }
}

#[test]
fn first_open_builds_and_writes_a_store() {
    let root = root("first");
    seed(&root);
    let store = root.join("store.redb");
    let (index, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Rebuilt(StoreRebuildReason::Missing));
    assert!(store.is_file());
    assert_eq!(count(&index), 20);
    assert!(index.has_store());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn second_open_loads_without_parsing() {
    let root = root("second");
    seed(&root);
    let store = root.join("store.redb");
    let (first, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    first.checkpoint().unwrap();
    drop(first);
    let (second, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Loaded);
    assert_eq!(count(&second), 20);
    assert_eq!(second.status().unwrap().build_parses, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_configuration_rebuilds() {
    let root = root("configuration");
    seed(&root);
    let store = root.join("store.redb");
    let (first, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    drop(first);
    let mut changed = definition();
    changed.sort_fields.push(FieldPath::parse("name").unwrap());
    let (second, outcome) = Index::open_persistent(
        &root,
        vec![changed.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(
        outcome,
        StoreOpenOutcome::Rebuilt(StoreRebuildReason::Configuration)
    );
    assert_eq!(count(&second), 20);
    drop(second);
    let (_, outcome) = Index::open_persistent(
        &root,
        vec![changed],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Loaded);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn format_version_mismatch_rebuilds() {
    let root = root("format");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    drop(index);
    {
        let database = redb::Database::open(&store).unwrap();
        let write = database.begin_write().unwrap();
        let mut meta = write.open_table(crate::index_store_keys::META).unwrap();
        meta.insert("format", 99u32.to_le_bytes().as_slice()).unwrap();
        drop(meta);
        write.commit().unwrap();
    }
    let (_, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(
        outcome,
        StoreOpenOutcome::Rebuilt(StoreRebuildReason::FormatVersion)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_checkpoint_rebuilds() {
    let root = root("interrupted");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    drop(index);
    {
        let database = redb::Database::open(&store).unwrap();
        let write = database.begin_write().unwrap();
        let mut meta = write.open_table(crate::index_store_keys::META).unwrap();
        meta.insert("complete", [0u8].as_slice()).unwrap();
        drop(meta);
        write.commit().unwrap();
    }
    let (_, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert!(matches!(outcome, StoreOpenOutcome::Rebuilt(StoreRebuildReason::Damaged(_))));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn damaged_store_rebuilds() {
    let root = root("damaged");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    drop(index);
    std::fs::write(&store, [0u8; 1024]).unwrap();
    let (rebuilt, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert!(matches!(outcome, StoreOpenOutcome::Rebuilt(_)));
    assert_eq!(count(&rebuilt), 20);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn locked_store_opens_without_store() {
    let root = root("locked");
    seed(&root);
    let store = root.join("store.redb");
    let (first, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    let (second, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert!(matches!(outcome, StoreOpenOutcome::WithoutStore(_)));
    assert_eq!(count(&second), 20);
    assert!(second.checkpoint().is_err());
    assert!(first.checkpoint().is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_does_not_touch_an_existing_store() {
    let root = root("untouched");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    drop(index);
    let before = std::fs::metadata(&store).unwrap();
    let length = before.len();
    let modified = before.modified().unwrap();
    let built = Index::build(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert_eq!(count(&built), 20);
    let after = std::fs::metadata(&store).unwrap();
    assert_eq!(after.len(), length);
    assert_eq!(after.modified().unwrap(), modified);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checkpoint_failure_keeps_the_index_usable() {
    let root = root("failure");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    index
        .0
        .store
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .fail_next_commit();
    assert!(index.checkpoint().is_err());
    assert_eq!(count(&index), 20);
    index.checkpoint().unwrap();
    drop(index);
    let (loaded, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Loaded);
    assert_eq!(count(&loaded), 20);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_edits_while_closed_are_picked_up() {
    let root = root("closed-edits");
    seed(&root);
    let store = root.join("store.redb");
    let (first, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    first.checkpoint().unwrap();
    drop(first);
    std::fs::write(root.join("data/000.jsonl"), b"{\"name\":\"edited\",\"n\":100}\n").unwrap();
    std::fs::write(root.join("data/new.jsonl"), b"{\"name\":\"new\",\"n\":101}\n").unwrap();
    std::fs::remove_file(root.join("data/001.jsonl")).unwrap();
    let (second, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Loaded);
    second.wait_for_reconciliation().unwrap();
    assert_eq!(count(&second), 20);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incremental_checkpoint_is_small() {
    let root = root("incremental");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    index.checkpoint().unwrap();
    index.set_fold_trigger_for_tests(1).unwrap();
    for number in 0..2usize {
        let path = root.join(format!("data/added-{number}.jsonl"));
        std::fs::write(&path, format!("{{\"name\":\"added-{number}\",\"n\":{number}}}\n")).unwrap();
        index.add_file(&path).unwrap();
    }
    index.checkpoint().unwrap();
    let path = root.join("data/added-2.jsonl");
    std::fs::write(&path, b"{\"name\":\"added-2\",\"n\":2}\n").unwrap();
    index.add_file(&path).unwrap();
    let report = index.checkpoint().unwrap();
    assert!(report.shards_written <= 1, "{report:?}");
    assert_eq!(report.main_shards_updated, 1);
    drop(index);
    let (reopened, outcome) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    assert_eq!(outcome, StoreOpenOutcome::Loaded);
    assert_eq!(count(&reopened), 23);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_views_are_unaffected_by_checkpoints() {
    let root = root("pinned");
    seed(&root);
    let store = root.join("store.redb");
    let (index, _) = Index::open_persistent(
        &root,
        vec![definition()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
        options(&store),
    )
    .unwrap();
    let pinned = index.view().unwrap();
    let path = root.join("data/pinned.jsonl");
    std::fs::write(&path, b"{\"name\":\"pinned\",\"n\":50}\n").unwrap();
    index.add_file(&path).unwrap();
    index.checkpoint().unwrap();
    assert_eq!(count(&index), 21);
    let query = Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 0,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(pinned.search(&query).unwrap().count, Some(20));
    std::fs::remove_dir_all(root).unwrap();
}
