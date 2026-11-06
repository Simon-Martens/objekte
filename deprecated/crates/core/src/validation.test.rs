use super::*;

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A pruned parse must accept exactly what a full parse accepts and keep the
/// listed members unchanged.
fn assert_matches_full(bytes: &[u8], kept: &[&str]) {
    let full = Document::parse(bytes);
    let pruned = Document::parse_pruned(bytes, &keys(kept));
    assert_eq!(
        pruned.state,
        full.state,
        "{}",
        String::from_utf8_lossy(bytes)
    );
    for key in kept {
        assert_eq!(pruned.data.get(key), full.data.get(key), "{key}");
    }
}

#[test]
fn pruned_parse_keeps_only_requested_members() {
    let doc = Document::parse_pruned(
        br#"{"a":1,"b":{"c":[1,2.50]},"d":"x","n":12345678901234567890.0}"#,
        &keys(&["b", "n"]),
    );
    assert_eq!(doc.state.status, RecordStatus::Pending);
    let object = doc.data.as_object().unwrap();
    assert_eq!(object.keys().collect::<Vec<_>>(), ["b", "n"]);
    assert_eq!(doc.data["n"].to_string(), "12345678901234567890.0");
    assert_eq!(doc.data["b"]["c"][1].to_string(), "2.50");
}

#[test]
fn pruned_parse_agrees_with_full_parse_on_malformed_skipped_members() {
    let deep = format!("{{\"a\":1,\"b\":{}{}}}", "[".repeat(200), "]".repeat(200));
    for bytes in [
        b"{\"a\":1,\"b\":\"\xff\"}".as_slice(),
        br#"{"a":1,"b":"\ud800"}"#,
        br#"{"a":1,"b":"\uDBFF"}"#,
        br#"{"a":1,"b":"\q"}"#,
        b"{\"a\":1,\"b\":\"x\x01y\"}",
        br#"{"a":1,"b":01}"#,
        br#"{"a":1,"b":1.}"#,
        br#"{"a":1,"b":[1,]}"#,
        br#"{"a":1} x"#,
        br#"{"\ud800":1,"a":2}"#,
        deep.as_bytes(),
    ] {
        assert_matches_full(bytes, &["a"]);
    }
}

#[test]
fn pruned_parse_matches_full_parse_for_valid_edge_cases() {
    let wide = format!("{{\"a\":1,\"b\":[{}]}}", vec!["[]"; 200].join(","));
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":1,"a":2}"#,
        br#"{"a":"\ud83d\ude00","b":"\u00e9"}"#,
        br#"  {"a":null}  "#,
        br#"{}"#,
        br#"[1,2]"#,
        br#"12"#,
        br#""a""#,
        br#"null"#,
        wide.as_bytes(),
    ] {
        assert_matches_full(bytes, &["a"]);
    }
    // Non-object roots fall back to the full value.
    let root = Document::parse_pruned(b"[1,2]", &keys(&["a"]));
    assert_eq!(root.data, serde_json::json!([1, 2]));
}
