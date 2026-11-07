use super::*;

#[test]
fn compaction_preserves_tokens_and_string_contents() {
    let source = b"{ \"x\" : [1e1000000, -0, \"a \\n b\", {\"x\":1, \"x\":2}] }\n";
    let mut compacted = Vec::new();
    compact_json(source, &mut compacted).unwrap();
    assert_eq!(
        compacted,
        b"{\"x\":[1e1000000,-0,\"a \\n b\",{\"x\":1,\"x\":2}]}"
    );
    assert_eq!(
        serde_json::from_slice::<IgnoredAny>(&compacted).is_ok(),
        serde_json::from_slice::<IgnoredAny>(source).is_ok()
    );
}

#[test]
fn json_string_escaping_has_no_terminal_control_bytes() {
    let mut encoded = Vec::new();
    write_json_string("red\u{1b}[31m\u{9b}\n\\\"", &mut encoded).unwrap();
    assert!(!encoded.contains(&0x1b));
    assert!(!encoded.contains(&0x9b));
    assert_eq!(
        serde_json::from_slice::<String>(&encoded).unwrap(),
        "red\u{1b}[31m\u{9b}\n\\\""
    );
}

#[test]
fn event_sequence_is_monotonic() {
    let mut output = EventWriter::new(Vec::new());
    output.emit(json!({"type": "run_start"})).unwrap();
    output.emit(json!({"type": "run_complete"})).unwrap();
    assert_eq!(output.sequence(), 2);
}
