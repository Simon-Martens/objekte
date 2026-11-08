use super::*;
use sha2::{Digest, Sha256};

fn test_root(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "objekte-config-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[test]
fn repository_layout_loads_sorted_definitions_and_protected_schema_targets() {
    let root = test_root("layout");
    let schemas = root.join(".objekte/collections");
    fs::create_dir_all(&schemas).unwrap();
    let authors = br#"{
        "type":"object",
        "properties":{"name":{"type":"string"}},
        "x-objekte":{"name":"authors","path":"authors","format":"jsonl","id":"generated"}
    }"#;
    let books = br#"{
        "type":"object",
        "properties":{
            "title":{"type":"string","x-index":true},
            "author":{"type":"string","x-relation":{"collection":"authors"}},
            "ghost":{"type":"string","x-relation":{"collection":"absent"}}
        },
        "x-language":"de",
        "x-objekte":{"name":"books","path":"books","format":"json","id":"filename"}
    }"#;
    let author_path = schemas.join("10-authors.schema.json");
    let book_path = schemas.join("20-books.schema.json");
    fs::write(&author_path, authors).unwrap();
    fs::write(&book_path, books).unwrap();
    fs::write(schemas.join("ignored.json"), b"not JSON").unwrap();
    fs::create_dir(root.join(".objekte/search-core-ascii.snapshot")).unwrap();

    let config = load_repository_configuration(&root).unwrap();
    assert_eq!(config.root, root.canonicalize().unwrap());
    assert_eq!(
        config.schema_files,
        vec![author_path.clone(), book_path.clone()]
    );
    assert_eq!(
        config.protected_files,
        vec![
            author_path.canonicalize().unwrap(),
            book_path.canonicalize().unwrap()
        ]
    );
    assert_eq!(config.definitions.len(), 2);
    assert_eq!(config.definitions[0].name, "authors");
    assert_eq!(config.definitions[0].source_path, Path::new("authors"));
    assert_eq!(config.definitions[0].format, SourceFormat::JsonLines);
    assert_eq!(config.definitions[0].identity, IdentityPolicy::Generated);
    assert_eq!(config.definitions[1].name, "books");
    assert_eq!(config.definitions[1].source_path, Path::new("books"));
    assert_eq!(config.definitions[1].format, SourceFormat::Json);
    assert_eq!(config.definitions[1].identity, IdentityPolicy::Filename);
    assert_eq!(config.definitions[1].sort_locale, "de");
    assert_eq!(config.definitions[1].schema.relations.len(), 1);
    assert_eq!(
        config.definitions[1].schema.source_fingerprint,
        Some(Sha256::digest(books).into())
    );
    assert_eq!(
        config.schema_fields,
        config
            .definitions
            .iter()
            .map(|definition| definition.schema.fields().len())
            .sum::<usize>()
    );
    assert_eq!(
        config.definitions[1].discovery.exclude_directories,
        vec![PathBuf::from(".objekte")]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_schema_reads_only_selected_file_and_reports_canonical_protection() {
    let root = test_root("explicit");
    fs::create_dir_all(root.join("schemas")).unwrap();
    let selected = root.join("selected.json");
    let bytes = br#"{
        "type":"object",
        "properties":{
            "title":{"type":"string"},
            "missing":{"type":"string","x-relation":{"collection":"absent"}}
        },
        "x-objekte":{"name":"ignored"}
    }"#;
    fs::write(&selected, bytes).unwrap();
    fs::write(root.join("schemas/nearby.json"), b"not JSON").unwrap();
    let aliased = root.join("schemas/../selected.json");
    let loaded = load_explicit_schema(&aliased, "files").unwrap();
    assert_eq!(loaded.path, selected.canonicalize().unwrap());
    assert!(loaded.ignored_repository_metadata);
    assert!(loaded.compiled.schema.relations.is_empty());
    assert_eq!(
        loaded.compiled.schema.source_fingerprint,
        Some(Sha256::digest(bytes).into())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_configuration_rejects_duplicate_collection_names() {
    let root = test_root("duplicates");
    let schemas = root.join(".objekte/collections");
    fs::create_dir_all(&schemas).unwrap();
    let document = br#"{
        "type":"object",
        "properties":{"name":{"type":"string"}},
        "x-objekte":{"name":"same","path":"data","format":"json","id":"filename"}
    }"#;
    fs::write(schemas.join("one.schema.json"), document).unwrap();
    fs::write(schemas.join("two.schema.json"), document).unwrap();
    let error = load_repository_configuration(&root).err().unwrap();
    assert!(error.contains("duplicate collection name same"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_configuration_never_searches_parent_directories() {
    let root = test_root("no-parent-search");
    let schemas = root.join(".objekte/collections");
    fs::create_dir_all(&schemas).unwrap();
    fs::write(
        schemas.join("files.schema.json"),
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string"}},
            "x-objekte":{"name":"files","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    assert!(load_repository_configuration(&root).is_ok());
    let error = load_repository_configuration(&child).err().unwrap();
    assert!(error.contains(".objekte/collections"), "{error}");
    fs::remove_dir_all(root).unwrap();
}
