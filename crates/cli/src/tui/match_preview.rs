//! Resolve an exact full-text hit path to the source value shown in the table.

use super::state::Hit;
use serde_json::Value;

fn value_at_path<'a>(value: &'a Value, target: &str, path: &str) -> Option<&'a Value> {
    if target == path
        || (path.is_empty()
            && target == "$value"
            && !matches!(value, Value::Object(_) | Value::Array(_)))
    {
        return Some(value);
    }
    if !path.is_empty() && !target.starts_with(path) {
        return None;
    }
    match value {
        Value::Object(fields) => fields.iter().find_map(|(name, child)| {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            value_at_path(child, target, &child_path)
        }),
        Value::Array(items) => {
            // Concrete hit paths carry array indices, so jump straight to the
            // matching item instead of walking every preceding element.
            let prefix = if path.is_empty() { "$value" } else { path };
            let suffix = target.strip_prefix(prefix)?.strip_prefix('[')?;
            let (index, _) = suffix.split_once(']')?;
            let index = index.parse::<usize>().ok()?;
            let child = items.get(index)?;
            let child_path = if path.is_empty() {
                format!("$value[{index}]")
            } else {
                format!("{path}[{index}]")
            };
            value_at_path(child, target, &child_path)
        }
        _ => None,
    }
}

pub(super) fn first_match(hit: &Hit) -> Option<String> {
    let Hit::Text(hit) = hit else { return None };
    let path = hit.matched_paths.first()?;
    let value = if path == "$raw" {
        hit.record
            .raw
            .as_deref()
            // The table renders 65 cells; avoid copying a whole malformed file.
            .map(|bytes| String::from_utf8_lossy(&bytes[..bytes.len().min(512)]))
            .map(|text| text.into_owned())
    } else {
        value_at_path(&hit.record.data, path, "").map(|value| match value {
            Value::String(text) => text.chars().take(512).collect(),
            value => value.to_string(),
        })
    };
    Some(value.map_or_else(|| path.clone(), |value| format!("{path}: {value}")))
}
