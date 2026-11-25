//! Focused declared-posting construction populations.
//!
//! This benchmark isolates value-selection and dictionary insertion from source
//! discovery, extraction, inference, and bigram construction.

use std::time::Instant;

use super::PostingCollection;

use crate::model::{ArrayValue, LogicalType, Number, RecordSlot, Value};
use crate::schema::{FieldDefinition, Schema};

#[test]
#[ignore = "release posting construction microbenchmark"]
fn bench_posting_populations() -> Result<(), String> {
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
                    object([
                        ("number", integer(number)),
                        ("text", text(number % 2 == 0)),
                        ("flag", Value::Bool(number % 2 == 0)),
                    ])
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
                    object([(
                        "values",
                        array([number, number + 1, number + 2].map(integer)),
                    )])
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
                    let items = (0_i64..12).map(|offset| {
                        object([
                            ("number", integer(number + offset)),
                            ("text", text(offset % 2 == 0)),
                            ("flag", Value::Bool(offset % 2 == 0)),
                        ])
                    });
                    object([("items", array(items))])
                })
                .collect(),
        ),
        (
            "duplicate_array",
            schema(&[(
                "values",
                LogicalType::Array(Box::new(LogicalType::String)),
                true,
            )]),
            (0..10_000)
                .map(|number| {
                    object([(
                        "values",
                        array(std::iter::repeat_n(text(number % 2 == 0), 32)),
                    )])
                })
                .collect(),
        ),
    ]
}

fn object<'a>(fields: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    Value::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(ArrayValue::from_unrestricted(values.into_iter().collect()))
}

fn integer(value: i64) -> Value {
    Value::Number(Number::integer(value as i128))
}

fn text(even: bool) -> Value {
    Value::String(if even { "A" } else { "B" }.to_owned())
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
