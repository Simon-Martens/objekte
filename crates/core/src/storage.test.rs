use super::*;
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::IdentityPolicy;
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

#[test]
fn round_trip_shared_multi_record_files_and_reject_corruption() {
    let root = std::env::temp_dir().join(format!("objekte-snapshot-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(
        root.join("data/multi.jsonl"),
        b"{\"x\":\"one\"}\n{\"x\":\"two\"}\n",
    )
    .unwrap();
    fs::write(root.join("data/empty.jsonl"), b"").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![FieldPath::parse("x").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect();
    let query = Query {
        collections: vec!["a".into()],
        fts: None,
        filter: Some(parse_filter("x = 'one'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        };
        let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert_eq!(built.build_parses, 2);
        let path = root.join(format!("{alphabet:?}.snapshot"));
        built.save_snapshot(&path).unwrap();
        let restored = load_snapshot(
            root.clone(),
            definitions.clone(),
            options,
            &crate::source::SourceRegistry::builtin(),
            &path,
        )
        .unwrap();
        assert_eq!(restored.files.files.len(), 2);
        assert_eq!(restored.files.files[0].collections.len(), 2);
        assert_eq!(restored.collections[0].generation.records.len(), 2);
        assert_eq!(
            restored.collections[0].generation.file_records.slots.len(),
            2
        );
        assert_eq!(
            built.search(&query).unwrap().count,
            restored.search(&query).unwrap().count
        );
        let other = RepositoryOptions {
            bigram_alphabet: if alphabet == BigramAlphabet::Ascii {
                BigramAlphabet::Unicode
            } else {
                BigramAlphabet::Ascii
            },
            ..options
        };
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                other,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .err()
            .unwrap()
            .contains("incompatible snapshot configuration")
        );
        let mut bytes = fs::read(&path).unwrap();
        let mut corrupted = bytes.clone();
        let tail = corrupted.len() - 1;
        corrupted[tail] ^= 0x01;
        fs::write(&path, &corrupted).unwrap();
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .err()
            .unwrap()
            .contains("snapshot checksum mismatch")
        );
        bytes.truncate(bytes.len() / 2);
        fs::write(&path, &bytes).unwrap();
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .is_err()
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_restores_unchanged_and_rebuilds_after_complete_file_changes() {
    let root = std::env::temp_dir().join(format!("objekte-reopen-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    let multi = root.join("data/multi.jsonl");
    fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("x").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions::default();
    let fresh = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!fresh.startup.restored);
    assert_eq!(fresh.build_parses, 2);
    let restored = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(restored.startup.restored);
    assert_eq!(restored.build_parses, 0);
    assert_eq!(restored.collections[0].generation.records.len(), 2);
    fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n{\"x\":\"c\"}\n").unwrap();
    let changed = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!changed.startup.restored);
    assert_eq!(changed.collections[0].generation.generation, 2);
    assert_eq!(changed.collections[0].generation.records.len(), 3);
    assert_eq!(
        changed.collections[0].generation.file_records.slots.len(),
        3
    );
    assert!(
        Repository::open_with_options(&root, definitions.clone(), options)
            .unwrap()
            .startup
            .restored
    );
    fs::write(root.join("data/extra.jsonl"), b"{\"x\":\"d\"}\n").unwrap();
    let added = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert_eq!(added.collections[0].generation.generation, 3);
    assert_eq!(added.collections[0].generation.records.len(), 4);
    fs::remove_file(multi).unwrap();
    let deleted = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert_eq!(deleted.collections[0].generation.generation, 4);
    assert_eq!(deleted.collections[0].generation.records.len(), 1);
    assert_eq!(deleted.files.files.len(), 1);
    let snapshot = snapshot_path(&root, BigramAlphabet::Ascii);
    let mut bytes = fs::read(&snapshot).unwrap();
    bytes[0] ^= 1;
    fs::write(&snapshot, bytes).unwrap();
    let rebuilt = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(!rebuilt.startup.restored);
    assert_eq!(rebuilt.collections[0].generation.records.len(), 1);
    fs::remove_dir_all(root).unwrap();
}
