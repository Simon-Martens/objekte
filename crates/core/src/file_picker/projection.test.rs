use super::*;
use crate::json::parse;
use std::fs;
use unicode_casefold::UnicodeCaseFold;

fn reference_value_text(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| reference_value_text(item, out)),
        Value::Object(items) => items
            .values()
            .for_each(|item| reference_value_text(item, out)),
        Value::String(value) => {
            for ch in value.chars() {
                out.push(if ch == '\n' || ch == '\r' {
                    '\u{1f}'
                } else {
                    ch
                });
            }
            out.push('\u{1f}');
        }
        Value::Number(value) => {
            out.push_str(&value.to_string());
            out.push('\u{1f}');
        }
        Value::Bool(value) => out.push_str(if *value { "true\u{1f}" } else { "false\u{1f}" }),
        Value::Null => out.push_str("null\u{1f}"),
    }
}

fn reference_indexed_value_text(value: &Value, out: &mut String) {
    reference_value_text(value, out);
    if !out.is_ascii() {
        let folded = out.case_fold().collect::<String>();
        if folded != out.to_ascii_lowercase() {
            out.push('\u{1f}');
            out.push_str(&folded);
        }
    }
}

#[test]
fn sonic_projection_matches_full_json_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let indexed = vec!["flag".into(), "score".into(), "title".into()];
    let present = vec![
        "array".into(),
        "flag".into(),
        "payload".into(),
        "score".into(),
        "title".into(),
    ];
    let view = ViewIndex::new(&indexed, &BTreeMap::new());
    let sort = super::SortIndex::new(&[], &BTreeMap::new(), None).unwrap();
    for raw in [
        r#"{"title":"first","t\u0069tle":"last","payload":{"a":1,"a":2},"array":[{"x":0,"x":3}],"score":9007199254740993,"flag":true}"#,
        r#"{"score":1e500,"title":"Straße\n🐈","flag":null,"payload":[1.0,-0,0.123456789012345678901234567890]}"#,
        r#"{"title":["invalid for scalar view"],"score":{"nested":1},"payload":false}"#,
        "null",
    ] {
        fs::write(root.join("a.json"), raw).unwrap();
        let source = Source::open(&root).unwrap();
        let stamp = source.inventory().unwrap().remove("a").unwrap();
        let projection =
            projected_values(&source, "a", &stamp, &indexed, &present, &view, &sort, true)
                .unwrap()
                .unwrap();
        let full = parse(raw).unwrap();
        let mut expected = String::new();
        reference_indexed_value_text(&full, &mut expected);
        assert_eq!(projection.bytes, expected.into_bytes(), "{raw}");
        let object = full.as_object();
        let expected_arrays = object.map_or_else(Vec::new, |fields| {
            fields
                .iter()
                .filter(|(_, value)| value.is_array())
                .map(|(name, _)| name.clone())
                .collect()
        });
        assert_eq!(projection.array_fields, expected_arrays, "{raw}");
        let expected_present = object.map_or_else(Vec::new, |fields| {
            fields
                .keys()
                .filter_map(|name| present.binary_search(name).ok())
                .collect()
        });
        assert_eq!(projection.present_fields, expected_present, "{raw}");
        let expected_typed: Vec<_> = indexed
            .iter()
            .enumerate()
            .filter_map(|(index, name)| {
                match object.and_then(|fields| fields.get(name)) {
                    Some(value) => TypedKey::from_value(value),
                    None => Some(TypedKey::Missing),
                }
                .map(|key| (index, key))
            })
            .collect();
        assert_eq!(projection.typed_values, expected_typed, "{raw}");
        assert_eq!(
            serde_json::to_value(projection.view_keys).unwrap(),
            serde_json::to_value(view.project("a", &full)).unwrap(),
            "{raw}"
        );
    }
}

#[test]
fn malformed_json_does_not_enter_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::write(root.join("a.json"), r#"{"title":"ok","ignored":[1,]}"#).unwrap();
    let source = Source::open(&root).unwrap();
    let stamp = source.inventory().unwrap().remove("a").unwrap();
    let view = ViewIndex::new(&[], &BTreeMap::new());
    let sort = super::SortIndex::new(&[], &BTreeMap::new(), None).unwrap();
    assert!(
        projected_values(&source, "a", &stamp, &[], &[], &view, &sort, true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn overwritten_deep_value_still_invalidates_index_projection() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let raw = format!(
                "{{\"title\":{}0{},\"title\":\"ok\"}}",
                "[".repeat(127),
                "]".repeat(127)
            );
            fs::write(root.join("a.json"), &raw).unwrap();
            let source = Source::open(&root).unwrap();
            let stamp = source.inventory().unwrap().remove("a").unwrap();
            let view = ViewIndex::new(&[], &BTreeMap::new());
            let sort = super::SortIndex::new(&[], &BTreeMap::new(), None).unwrap();
            assert!(parse(&raw).is_err());
            assert!(
                projected_values(&source, "a", &stamp, &[], &[], &view, &sort, true)
                    .unwrap()
                    .is_none()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
