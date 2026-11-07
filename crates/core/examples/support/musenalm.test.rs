use super::*;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::query::Query;
use objekte_search_core::reference::bind_filter;
use objekte_search_core::relation::RelationCardinality;
use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::source::{SourceFormat, SourceRegistry};
use objekte_search_core::workload::load_manifest;

#[test]
fn fixture_schemas_bind_every_fixed_request() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let definitions = load_definitions(&root).unwrap();
    assert_eq!(definitions.len(), 7);
    let cases =
        load_manifest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json"))
            .unwrap();
    for case in cases {
        let query = Query::from_workload(&case).unwrap();
        for definition in definitions
            .iter()
            .filter(|d| case.collection == "*" || case.collection == d.name)
        {
            if let Some(filter) = &query.filter {
                bind_filter(filter, &definition.schema)
                    .unwrap_or_else(|e| panic!("{} {}: {e}", case.id, definition.name));
            }
            if let Some(fts) = &query.fts {
                for field in &fts.fields {
                    definition.schema.field(&field.path).unwrap();
                }
            }
        }
    }
}

#[test]
fn fractional_numbering_remains_numeric() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/musenalm/data/contents/00mc7w4nobyg3xt.json");
    let extracted = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap()
        .reader()
        .parse(&std::fs::read(path).unwrap())
        .unwrap();
    let Value::Object(fields) = extracted.value else {
        panic!("object expected")
    };
    assert_eq!(
        fields.get("numbering"),
        Some(&Value::Number(Number::float(32.01).unwrap()))
    );
}

#[test]
fn schema_adapter_carries_nested_item_nullability() {
    let schema = serde_json::json!({
        "type": "array",
        "items": {"type": ["null", "array"], "items": {"type": "number"}}
    });
    assert_eq!(array_element_nullability(&schema), [true, false]);
}

#[test]
fn extended_schema_declares_only_annotated_id_relations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let (historical, historical_schemas) = load_definitions_with_schemas(&root).unwrap();
    let (extended, extended_schemas) = load_extended_definitions_with_schemas(&root).unwrap();
    assert!(
        historical
            .iter()
            .all(|item| item.schema.relations().is_empty())
    );
    assert!(historical_schemas.iter().all(|item| item.adapter.is_none()));
    assert!(
        extended_schemas
            .iter()
            .all(|item| item.adapter.as_deref() == Some("musenalm_relations_v1"))
    );
    assert_eq!(
        historical_schemas
            .iter()
            .map(|item| &item.schema)
            .collect::<Vec<_>>(),
        extended_schemas
            .iter()
            .map(|item| &item.schema)
            .collect::<Vec<_>>()
    );
    let historical_digest = schema_manifest(&historical_schemas).unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        historical_digest,
        "38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9"
    );
    assert_ne!(
        historical_digest,
        schema_manifest(&extended_schemas).unwrap()["sha256"]
    );

    let expected = [
        (
            "contents",
            "contributors.agents",
            "agents",
            RelationCardinality::Many,
        ),
        ("contents", "entry", "entries", RelationCardinality::One),
        (
            "entries",
            "agents.agent",
            "agents",
            RelationCardinality::Many,
        ),
        ("entries", "places", "places", RelationCardinality::Many),
        ("series", "entries", "entries", RelationCardinality::Many),
        (
            "series",
            "related_entries.entry",
            "entries",
            RelationCardinality::Many,
        ),
    ];
    assert_eq!(
        extended
            .iter()
            .map(|item| item.schema.relations().len())
            .sum::<usize>(),
        expected.len()
    );
    for (collection, path_text, target, cardinality) in expected {
        let path = path_text.split('.').map(str::to_owned).collect::<Vec<_>>();
        let definition = extended
            .iter()
            .find(|item| item.name == collection)
            .unwrap();
        let relation = definition.schema.relations().get(&path).unwrap();
        assert_eq!(relation.target_collection, target);
        assert_eq!(relation.cardinality, cardinality);
        assert_eq!(
            relation.reverse_alias.as_deref(),
            Some(format!("{collection}__{}", path.join("__")).as_str())
        );
        let field = definition
            .schema
            .field(&FieldPath::parse(path_text).unwrap())
            .unwrap();
        assert!(field.indexed);
        assert_eq!(
            field.kind,
            if path_text == "places" || path_text == "entries" {
                LogicalType::Array(Box::new(LogicalType::Id))
            } else {
                LogicalType::Id
            }
        );
    }
}

#[test]
fn extended_schema_accepts_explicit_alias_on_nullable_array_item() {
    let document = serde_json::json!({
        "properties": {
            "links": {
                "type": ["array", "null"],
                "items": {
                    "type": "string",
                    "x-relation": {"collection": "targets", "reverse_alias": "chosen_alias"}
                }
            }
        }
    });
    let mut schema = Schema::default();
    let mut relations = Vec::new();
    register_properties(
        &document,
        &[],
        false,
        &mut SchemaRegistration {
            indexed_paths: &HashSet::new(),
            schema: &mut schema,
            mode: SchemaMode::ExtendedRelations,
            source_collection: "sources",
            relations: &mut relations,
        },
    )
    .unwrap();
    for relation in relations {
        schema.register_relation(relation).unwrap();
    }
    let relation = schema.relations().get(&vec!["links".to_owned()]).unwrap();
    assert_eq!(relation.reverse_alias.as_deref(), Some("chosen_alias"));
    assert_eq!(relation.cardinality, RelationCardinality::Many);
    assert_eq!(
        schema
            .field(&FieldPath::parse("links").unwrap())
            .unwrap()
            .kind,
        LogicalType::Array(Box::new(LogicalType::Id))
    );
}

#[test]
#[ignore = "builds the complete Musenalm corpus"]
fn extended_schema_builds_real_forward_and_backlink() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let definitions = load_extended_definitions(&root).unwrap();
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    let source = "000dl3mfclrmjwu";
    let target = "jv829b9kekdb5d3";
    let outgoing = repository
        .relation_outgoing_page("contents", &["entry".to_owned()], source, 0, 10)
        .unwrap();
    assert!(
        outgoing
            .references
            .iter()
            .any(|reference| { reference.target_id == target && reference.resolved.is_some() })
    );
    let backlink = repository
        .relation_backlink_page("entries", "contents__entry", target, 0, 10)
        .unwrap();
    assert!(backlink.sources.iter().any(|record| record.id == source));
}
