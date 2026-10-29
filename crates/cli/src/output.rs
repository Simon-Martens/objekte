//! Table, JSON, and NDJSON presentation.

use objekte_core::ViewResult;
use serde_json::{Value, json};

pub(super) fn metrics_in_milliseconds(metrics: Value) -> Value {
    let Value::Object(fields) = metrics else {
        unreachable!("library metrics serialize as an object")
    };
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| {
                if let Some(phase) = name.strip_suffix("_nanos") {
                    let nanos = value.as_u64().expect("library timers are u64 nanoseconds");
                    (format!("{phase}_ms"), json!(nanos as f64 / 1_000_000.0))
                } else {
                    (name, value)
                }
            })
            .collect(),
    )
}

fn cell(value: &Value) -> String {
    let text = value.to_string();
    let mut chars = text.chars();
    let mut out = chars.by_ref().take(100).collect::<String>();
    if chars.next().is_some() {
        out.push('…')
    }
    out
}

pub(super) fn print_view(
    view: &ViewResult,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        "json" => {
            let mut value = serde_json::to_value(view)?;
            value["metrics"] = metrics_in_milliseconds(value["metrics"].take());
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        "ndjson" => {
            for row in &view.rows {
                let mut value = serde_json::to_value(row)?;
                value["state"] = serde_json::to_value(&view.states[&row.id])?;
                println!("{}", serde_json::to_string(&value)?)
            }
        }
        "table" => {
            let mut headers = vec!["ID".to_owned(), "STATUS".to_owned()];
            headers.extend(view.columns.iter().map(|s| cell(&json!(s))));
            let rows = view
                .rows
                .iter()
                .map(|r| {
                    [
                        cell(&json!(r.id)),
                        cell(&serde_json::to_value(view.states[&r.id].status).unwrap()),
                    ]
                    .into_iter()
                    .chain(view.columns.iter().map(|f| {
                        r.values
                            .get(f)
                            .map(cell)
                            .unwrap_or_else(|| "<missing>".into())
                    }))
                    .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let widths = (0..headers.len())
                .map(|i| {
                    std::iter::once(headers[i].chars().count())
                        .chain(rows.iter().map(|r| r[i].chars().count()))
                        .max()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let print = |row: &[String]| {
                println!(
                    "{}",
                    row.iter()
                        .enumerate()
                        .map(|(i, s)| format!("{}{}", s, " ".repeat(widths[i] - s.chars().count())))
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            };
            print(&headers);
            println!(
                "{}",
                widths
                    .iter()
                    .map(|w| "-".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("-+-")
            );
            for row in rows {
                print(&row)
            }
            if let Some(total) = view.total {
                println!("{} rows shown; {total} matching records", view.rows.len())
            }
        }
        _ => return Err("output must be table, json, or ndjson".into()),
    }
    Ok(())
}
