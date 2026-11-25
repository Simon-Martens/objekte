use super::*;

#[test]
fn json_string_tokens_round_trip_quotes_controls_and_unicode() {
    let value = "\"\\\t\n\r\u{8}\u{c}\u{0}\u{1f}é😀";
    let mut literal = String::new();
    write_json_string_literal(&mut literal, value);
    assert_eq!(literal, r#""\"\\\t\n\r\b\f\u0000\u001fé😀""#);
    assert_eq!(unescape_json_string(&literal).unwrap(), value);
    assert_eq!(unescape_json_string(r#""\ud83d\ude00\/""#).unwrap(), "😀/");
}

#[test]
fn json_string_tokens_reject_invalid_escapes() {
    for token in [
        r#""\uABQ0""#,
        r#""\uD800""#,
        r#""\uDC00""#,
        r#""\uD800\u0000""#,
        r#""\q""#,
        r#""\""#,
        "\"newline\n\"",
        r#""one" "two""#,
    ] {
        assert!(unescape_json_string(token).is_err(), "{token:?}");
    }
}
