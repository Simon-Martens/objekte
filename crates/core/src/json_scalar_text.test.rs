use super::*;

#[test]
fn decoded_scalar_text_uses_values_not_names_or_container_syntax() {
    let bytes = br#"{"name_only": ["a\n", 1.0, 1e0, -0, true, false, null, {"v":"\u00df"}]}"#;
    let all = scalar_segments(bytes, false, true).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["a\n", "1.0", "1e0", "-0", "true", "false", "null", "ß"]
    );
    assert!(
        all.iter()
            .all(|segment| segment.path.first().is_some_and(|part| part == "name_only"))
    );
}

#[test]
fn all_text_keeps_duplicate_members_but_selected_text_uses_last_wins() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"array":[1,"two"]}"#;
    let all = scalar_segments(bytes, false, true).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["old", "new", "1", "two"]
    );
    let resolved = scalar_segments(bytes, true, true).unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["new", "1", "two"]
    );
}

#[test]
fn fresh_build_decodes_value_and_scalar_text_in_one_walk() {
    let documents: &[&[u8]] = &[
        br#"{"title":"first","title":"last","mixed":[null,true,1,1.0,1e0,-0,{"x":"\u00df"}]}"#,
        br#"["first",123,false,null,{"nested":["\ud83d\ude80",1e1000000]}]"#,
        br#""a root string""#,
        b"1234567890123456789012345678901234567890",
        b"true",
        b"null",
        b"[]",
        b"{}",
    ];
    for bytes in documents {
        let (value, text) = build_record(bytes).unwrap();
        let expected = Value::from_json(serde_json::from_slice(bytes).unwrap()).unwrap();
        assert_eq!(value, expected, "{}", String::from_utf8_lossy(bytes));
        assert_eq!(
            text.iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            scalar_segments(bytes, false, false)
                .unwrap()
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>(),
            "{}",
            String::from_utf8_lossy(bytes),
        );
    }
}

#[test]
fn fresh_build_rejects_malformed_json_without_a_validated_first_pass() {
    for bytes in [
        b"[1,]".as_slice(),
        b"{\"a\":1,}".as_slice(),
        b"truefalse".as_slice(),
        b"1e".as_slice(),
        b"\"bad\\q\"".as_slice(),
    ] {
        assert!(build_record(bytes).is_err(), "{bytes:?}");
    }
}
