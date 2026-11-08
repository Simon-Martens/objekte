use super::*;

#[test]
fn full_field_path_resolves_nested_json_without_display_clipping() {
    let value = serde_json::json!({"nested": [{"title": "a".repeat(9000)}]});
    let found = find_json_path(&value, "$", "$.nested[0].title").unwrap();
    assert_eq!(found.as_str().unwrap().len(), 9000);
}

#[test]
fn bounded_batch_status_extraction_skips_large_hits() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("events.ndjson");
    let huge = "x".repeat(32 * 1024);
    std::fs::write(&output, format!(
        "{{\"type\":\"query_start\",\"query_id\":1,\"line\":4,\"input\":\"a = 1\"}}\n{{\"type\":\"hit\",\"data\":\"{huge}\"}}\n{{\"type\":\"query_error\",\"query_id\":1,\"error\":\"invalid\"}}\n"
    )).unwrap();
    let rows = batch_rows(&output).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].line, 4);
    assert_eq!(rows[0].query, "a = 1");
    assert_eq!(rows[0].error.as_deref(), Some("invalid"));
}
