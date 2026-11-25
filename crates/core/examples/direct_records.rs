//! Build and query an in-memory index using core alone.

use std::collections::{BTreeMap, HashMap};

use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::schema::Schema;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default())?;
    let provider = builder.register_records(collection, "example.entries", 1)?;
    let index = builder.build()?;
    let writer = index.ingestion().writer(provider)?;
    let mut batch = writer.batch();
    for (id, score) in [("example-1", 9_007_199_254_740_993), ("example-2", 2)] {
        batch.insert(
            id,
            Value::Object(BTreeMap::from([
                (
                    "section.\"score\"".into(),
                    Value::Number(Number::integer(score)),
                ),
                ("label".into(), Value::String("direct\nrecord".into())),
            ])),
        )?;
    }
    writer
        .submit(batch)
        .map_err(|_| "batch admission refused")?
        .wait_published()?;

    let request = Query {
        collections: vec!["entries".into()],
        filter: Some(parse_filter(
            r#"["section.\"score\""] >= $minimum && label = "direct\nrecord""#,
        )?),
        fts: None,
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    let request = request.with_parameters(&HashMap::from([(
        "minimum".into(),
        Value::Number(Number::integer(9_007_199_254_740_993)),
    )]))?;
    let results = index.view()?.search(&request)?;
    assert_eq!(results.count, Some(1));
    assert!(results.hits[0].file.is_none());
    assert_eq!(results.hits[0].record_id, "example-1");
    println!("{}", results.hits[0].record_id);
    Ok(())
}
