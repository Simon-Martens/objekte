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
