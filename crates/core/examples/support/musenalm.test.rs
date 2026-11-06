use super::*;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::query::Query;
use objekte_search_core::reference::bind_filter;
use objekte_search_core::source::{SourceFormat, reader};
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
    let extracted = reader(SourceFormat::Json)
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
