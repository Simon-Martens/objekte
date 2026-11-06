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
        assert_eq!(built.build_parses, 4);
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
        assert!(
            restored
                .files
                .files
                .iter()
                .all(|file| file.status == crate::inventory::FileStatus::Valid
                    && file.diagnostics.is_empty())
        );
        assert_eq!(restored.collections[0].generation.records.len(), 2);
        assert!(
            restored.collections[0]
                .generation
                .records
                .iter()
                .all(
                    |record| record.class == crate::inventory::RecordClass::Logical
                        && record.diagnostic_ref == crate::inventory::DiagnosticRef(0)
                )
        );
        assert_eq!(
            restored.collections[0].generation.file_records.slots.len(),
            2
        );
        assert_eq!(
            built.search(&query).unwrap().count.unwrap(),
            restored.search(&query).unwrap().count.unwrap()
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
fn seeded_snapshot_mutations_and_truncations_are_rejected_without_panics() {
    const SEED: u64 = 0x5A9A_2026_1106;
    let root = std::env::temp_dir().join(format!("objekte-snapshot-fuzz-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/one.json"), br#"{"x":"one"}"#).unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions {
        snapshot_mode: crate::repository::SnapshotMode::Disabled,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let snapshot = root.join("snapshot.bin");
    built.save_snapshot(&snapshot).unwrap();
    let original = fs::read(&snapshot).unwrap();
    let mut state = SEED;
    for case in 0..64 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let index = state as usize % original.len();
        let mut bytes = original.clone();
        bytes[index] ^= 1 << (case % 8);
        fs::write(&snapshot, bytes).unwrap();
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
        }));
        assert!(
            loaded.is_ok() && loaded.unwrap().is_err(),
            "seed={SEED:#x} case={case} index={index}"
        );
    }
    for size in [0, 1, 8, 16, original.len() / 2, original.len() - 1] {
        fs::write(&snapshot, &original[..size]).unwrap();
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
        }));
        assert!(
            loaded.is_ok() && loaded.unwrap().is_err(),
            "seed={SEED:#x} truncated_size={size}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checksummed_snapshot_with_invalid_diagnostic_reference_is_rejected() {
    use std::io::Cursor;
    let root = std::env::temp_dir().join(format!("objekte-snapshot-ref-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/bad.json"), br#"{"title":"bad""#).unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions {
        snapshot_mode: crate::repository::SnapshotMode::Disabled,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let snapshot = root.join("snapshot.bin");
    built.save_snapshot(&snapshot).unwrap();
    let bytes = fs::read(&snapshot).unwrap();
    let header_size = MAGIC.len() + 4 + 4 + 32 + 4;
    let mut reader = Cursor::new(&bytes);
    reader.set_position(header_size as u64);
    let files: Vec<EncodedFile> = read_section(&mut reader, 0).unwrap();
    let mut collection: EncodedCollection = read_section(&mut reader, 1).unwrap();
    assert_eq!(collection.generation.records.len(), 1);
    collection.generation.records[0].diagnostic_ref = crate::inventory::DiagnosticRef(42);
    let mut rewritten = bytes[..header_size].to_vec();
    write_section(&mut rewritten, 0, &files).unwrap();
    write_section(&mut rewritten, 1, &collection).unwrap();
    fs::write(&snapshot, rewritten).unwrap();
    let error = load_snapshot(
        root.clone(),
        definitions,
        options,
        &crate::source::SourceRegistry::builtin(),
        &snapshot,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("record range or revision mismatch"),
        "{error}"
    );
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
