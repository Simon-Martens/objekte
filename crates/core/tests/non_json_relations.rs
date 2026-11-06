use std::collections::BTreeMap;
use std::sync::Arc;

use objekte_search_core::inventory::ByteRange;
use objekte_search_core::model::{LogicalType, Value};
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, RegexOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::{
    ExtractedRecord, SourceFormat, SourceReader, SourceRegistry, TextSegment,
};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase};

struct PipeReader;

impl SourceReader for PipeReader {
    fn name(&self) -> &'static str {
        "query_feature_pipe"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let parts = text.split('|').collect::<Vec<_>>();
        let [id, title, issued, entry] = parts.as_slice() else {
            return Err("pipe record needs four fields".into());
        };
        let mut fields = BTreeMap::from([
            ("id".into(), Value::String((*id).into())),
            ("title".into(), Value::String((*title).into())),
        ]);
        if *issued != "-" {
            fields.insert("issued".into(), Value::String((*issued).into()));
        }
        if *entry != "-" {
            fields.insert("entry".into(), Value::String((*entry).into()));
        }
        Ok(ExtractedRecord {
            value: Value::Object(fields),
            text: vec![TextSegment {
                path: vec!["title".into()],
                text: (*title).into(),
            }],
        })
    }
}

fn add_field(schema: &mut Schema, name: &str, kind: LogicalType) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: name == "id" || name == "title",
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    add_field(&mut entries, "id", LogicalType::Id);
    add_field(&mut entries, "title", LogicalType::String);
    add_field(&mut entries, "issued", LogicalType::Date);
    let mut mentions = Schema::default();
    add_field(&mut mentions, "id", LogicalType::Id);
    add_field(&mut mentions, "title", LogicalType::String);
    add_field(&mut mentions, "entry", LogicalType::Id);
    mentions
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("mentions".into()),
        })
        .unwrap();
    [("entries", entries), ("mentions", mentions)]
        .into_iter()
        .map(|(name, schema)| CollectionDefinition {
            name: name.into(),
            source_path: name.into(),
            format: SourceFormat::registered("pipe"),
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect()
}

fn request() -> Query {
    Query::from_workload(&WorkloadCase {
        id: "non-json-relations".into(),
        collection: "mentions".into(),
        fts: Some("^Hy.*n$".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Regex(RegexOptions::default()),
        filter: Some("entry.issued = 2024-02".into()),
        fts_fields: vec!["entry.title".into()],
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Logical,
        evidence: true,
    })
    .unwrap()
}

#[test]
fn registered_non_json_reader_supports_typed_relation_temporal_and_regex_requests() {
    let root = std::env::temp_dir().join(format!(
        "objekte-non-json-relations-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    std::fs::create_dir_all(root.join("mentions")).unwrap();
    for (file, record) in [
        ("entries/a.pipe", "A|Hymn|2024-02-29|-"),
        ("entries/b.pipe", "B|Festival|2024-03-01|-"),
        ("mentions/m1.pipe", "m1|Note|-|A"),
        ("mentions/m2.pipe", "m2|Other|-|B"),
    ] {
        std::fs::write(root.join(file), record).unwrap();
    }
    let mut sources = SourceRegistry::builtin();
    sources
        .register("pipe", 1, "pipe", Arc::new(PipeReader))
        .unwrap();
    let query = request();
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built =
            Repository::open_with_sources(&root, definitions(), options, sources.clone()).unwrap();
        let restored =
            Repository::open_with_sources(&root, definitions(), options, sources.clone()).unwrap();
        assert!(restored.startup().restored);
        for repository in [&built, &restored] {
            let actual = repository.search(&query).unwrap();
            let reference = repository.search_reference(&query).unwrap();
            assert_eq!(actual.hits, reference.hits);
            assert_eq!(actual.count, Some(1));
            assert_eq!(actual.hits[0].record_id, "m1");
            assert!(
                actual.hits[0]
                    .matched_fields
                    .contains(&"entry.title".into())
            );
            assert!(
                repository
                    .search(&query)
                    .unwrap()
                    .counters
                    .verified_result_hit
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
