//! Build and query an in-memory index using core alone.

use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::Value;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::schema::Schema;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default())?;
    let provider = builder.register_records(collection, "example.entries", 1)?;
    let index = builder.build()?;
    let writer = index.ingestion().writer(provider)?;
    let mut batch = writer.batch();
    batch.insert("example-1", Value::String("direct record".into()))?;
    writer
        .submit(batch)
        .map_err(|_| "batch admission refused")?
        .wait_published()?;

    let results = index.view()?.search(&Query {
        collections: vec!["entries".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    })?;
    assert_eq!(results.count, Some(1));
    assert!(results.hits[0].file.is_none());
    println!("{}", results.hits[0].record_id);
    Ok(())
}
