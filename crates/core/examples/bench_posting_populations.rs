//! Focused declared-posting construction populations.
//!
//! This benchmark isolates value-selection and dictionary insertion from source
//! discovery, extraction, inference, and bigram construction.

use std::time::Instant;

use objekte_search_core::index::posting::PostingCollection;
use objekte_search_core::model::{LogicalType, RecordSlot, Value};
use objekte_search_core::schema::{FieldDefinition, Schema};

fn main() -> Result<(), String> {
    for (name, schema, records) in populations() {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let mut postings = PostingCollection::new(&schema);
            for (slot, record) in records.iter().enumerate() {
                postings.add(RecordSlot(slot as u32), record)?;
            }
            postings.finish_stats();
            samples.push(started.elapsed().as_nanos() as u64);
        }
        samples.sort_unstable();
        println!(
            "{{\"population\":\"{name}\",\"records\":{},\"median_ns\":{}}}",
            records.len(),
            samples[samples.len() / 2]
        );
    }
    Ok(())
}

fn populations() -> Vec<(&'static str, Schema, Vec<Value>)> {
    vec![
        (
            "singleton",
            schema(&[
                ("number", LogicalType::Number, false),
                ("text", LogicalType::String, false),
                ("flag", LogicalType::Bool, false),
            ]),
            (0_i64..50_000)
                .map(|number| {
                    Value::from_json(serde_json::json!({
                        "number": number,
                        "text": if number % 2 == 0 { "A" } else { "B" },
                        "flag": number % 2 == 0
                    }))
                    .unwrap()
                })
                .collect(),
        ),
        (
            "short_array",
            schema(&[(
                "values",
                LogicalType::Array(Box::new(LogicalType::Number)),
                true,
            )]),
            (0_i64..25_000)
                .map(|number| {
                    Value::from_json(serde_json::json!({
                        "values": [number, number + 1, number + 2]
                    }))
                    .unwrap()
                })
                .collect(),
        ),
        (
            "repeated_prefix",
            schema(&[
                (
                    "items",
                    LogicalType::Array(Box::new(LogicalType::Object)),
                    true,
                ),
                ("items.number", LogicalType::Number, true),
                ("items.text", LogicalType::String, true),
                ("items.flag", LogicalType::Bool, true),
            ]),
            (0_i64..5_000)
                .map(|number| {
                    let items = (0_i64..12)
                        .map(|offset| {
                            serde_json::json!({
                                "number": number + offset,
                                "text": if offset % 2 == 0 { "A" } else { "B" },
                                "flag": offset % 2 == 0
                            })
                        })
                        .collect::<Vec<_>>();
                    Value::from_json(serde_json::json!({ "items": items })).unwrap()
                })
                .collect(),
        ),
    ]
}

fn schema(fields: &[(&str, LogicalType, bool)]) -> Schema {
    let mut schema = Schema::default();
    for (path, kind, multi) in fields {
        schema
            .register_field(FieldDefinition {
                path: path.split('.').map(str::to_owned).collect(),
                kind: kind.clone(),
                required: false,
                nullable: true,
                indexed: true,
                multi: *multi,
                element_nullable: vec![true; path.split('.').count()],
            })
            .unwrap();
    }
    schema
}
