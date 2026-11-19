use super::*;
use crate::model::Value as LogicalValue;

#[test]
fn plain_schema_distinguishes_manual_indexes_and_optouts() {
    let document = serde_json::json!({
        "type":"object",
        "properties":{
            "id":{"type":"string","index":true},
            "year":{"type":"integer"},
            "active":{"type":"boolean","index":true},
            "body":{"type":"string","index":false}
        },
        "x-index-paths":["year"],
        "x-sort-indexes":[[{"field":"year","direction":"asc"}]]
    });
    let compiled = compile_schema(&document, "files").unwrap();
    assert_eq!(compiled.schema.fields().len(), 4);
    for name in ["id", "year", "active"] {
        assert!(compiled.schema.fields()[&vec![name.into()]].indexed);
    }
    assert!(compiled.schema.is_auto_index_prohibited(&["body".into()]));
    assert_eq!(compiled.sort_fields[0].display(), "year");
    assert!(compile_schema(&serde_json::json!({"type":"object","properties":{"body":{"type":"string","index":false}},"x-index-paths":["body"]}), "files").is_err());
}

#[test]
fn local_references_nullable_arrays_and_root_values_are_explicit() {
    let document = serde_json::json!({
        "type":"object",
        "$defs":{"identifier":{"type":["string","null"]}},
        "properties":{"ids":{"type":"array","items":{"$ref":"#/$defs/identifier"}}}
    });
    let compiled = compile_schema(&document, "files").unwrap();
    assert_eq!(
        compiled.schema.fields()[&vec!["ids".into()]].element_nullable,
        [true]
    );
    let mut valid = LogicalValue::from_json(serde_json::json!({"ids":["x",null]})).unwrap();
    compiled.schema.coerce_record(&mut valid).unwrap();
    let mut invalid = LogicalValue::from_json(serde_json::json!({"ids":[3]})).unwrap();
    assert!(compiled.schema.coerce_record(&mut invalid).is_err());

    let scalar =
        compile_schema(&serde_json::json!({"type":"string","index":true}), "files").unwrap();
    let mut text = LogicalValue::String("x".into());
    scalar.schema.coerce_record(&mut text).unwrap();
    let mut number = LogicalValue::from_json(serde_json::json!(3)).unwrap();
    assert!(scalar.schema.coerce_record(&mut number).is_err());
    assert!(scalar.schema.fields()[&Vec::<String>::new()].indexed);

    for document in [
        serde_json::json!({"type":["string","number"]}),
        serde_json::json!({"$ref":"https://example.invalid/schema"}),
        serde_json::json!({"$ref":"#/$defs/self","$defs":{"self":{"$ref":"#/$defs/self"}}}),
    ] {
        assert!(compile_schema(&document, "files").is_err());
    }
}

#[test]
fn composite_sort_is_not_flattened_into_an_unrelated_order() {
    let document = serde_json::json!({
        "type":"object",
        "properties":{"year":{"type":"number"},"title":{"type":"string"}},
        "x-sort-indexes":[[{"field":"year","direction":"desc"},{"field":"title","direction":"asc"}]]
    });
    let compiled = compile_schema(&document, "files").unwrap();
    assert_eq!(compiled.sort_fields.len(), 2);
    assert_eq!(compiled.schema.composite_sorts.len(), 1);
    assert!(compiled.schema.composite_sorts[0][0].descending);
    assert!(!compiled.schema.composite_sorts[0][1].descending);
}

#[test]
fn root_and_nested_arrays_register_item_properties() {
    let document = serde_json::json!({
        "type":"array",
        "items":{"type":"array","items":{"type":"object","properties":{
            "code":{"type":"string","index":true},
            "count":{"type":"integer"}
        }}},
        "x-index-paths":["count"]
    });
    let compiled = compile_schema(&document, "files").unwrap();
    assert!(compiled.schema.fields()[&vec!["code".into()]].indexed);
    assert!(compiled.schema.fields()[&vec!["count".into()]].indexed);
    assert!(compiled.schema.fields()[&vec!["code".into()]].multi);
    let mut valid = LogicalValue::from_json(serde_json::json!([[{"code":"A","count":1}]])).unwrap();
    compiled.schema.coerce_record(&mut valid).unwrap();
    let mut invalid = LogicalValue::from_json(serde_json::json!([[{"code":7,"count":1}]])).unwrap();
    assert!(compiled.schema.coerce_record(&mut invalid).is_err());
}

#[test]
fn explicit_schema_load_uses_only_the_requested_file() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-schema-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let selected = root.join("selected.json");
    std::fs::write(
        &selected,
        br#"{"type":"object","properties":{"title":{"type":"string","index":true}}}"#,
    )
    .unwrap();
    let compiled = load_schema_file(&selected, "documents").unwrap();
    assert!(compiled.schema.fields()[&vec!["title".into()]].indexed);
    assert!(compiled.schema.source_fingerprint.is_some());

    let unselected = root.join("other.json");
    assert!(
        load_schema_file(&unselected, "documents")
            .err()
            .unwrap()
            .contains("other.json")
    );
    std::fs::write(&selected, br#"{"type":"object","properties":42}"#).unwrap();
    assert!(
        load_schema_file(&selected, "documents")
            .err()
            .unwrap()
            .contains("selected.json")
    );
    std::fs::remove_dir_all(root).unwrap();
}
