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

#[test]
fn query_extraction_matches_resolved_values_and_source_scalar_text() {
    let documents: &[&[u8]] = &[
        br#"{"title":"old","title":{"inner":"new"},"n":1e0,"items":[true,null,"\ud83d\ude80"]}"#,
        br#"{"a":{"x":"lost"},"a":{},"b":[{"x":"first"},{"x":"last"}],"a.b":"dot"}"#,
        br#"{"\u0061":"first","a":["second",{"nested":-0}],"empty":[]}"#,
        br#"[{"a":"one"},{"a":"two"},12.5,false,null]"#,
        br#""root\u00df""#,
        b"1234567890123456789012345678901234567890",
        b"true",
        b"null",
        b"{}",
        b"[]",
    ];
    for bytes in documents {
        for selected in [
            None,
            Some(HashSet::from(["title".into(), "a".into(), "n".into()])),
        ] {
            for collect_text in [false, true] {
                for resolved_text_only in [false, true] {
                    let projection = ScanProjection {
                        top_level: selected.clone(),
                        collect_text,
                        resolved_text_only,
                    };
                    let (actual_value, actual_text) =
                        extract_query_record(bytes, &projection, false).unwrap();
                    let (trusted_value, trusted_text) =
                        extract_query_record(bytes, &projection, true).unwrap();
                    assert_eq!(actual_value, trusted_value, "{bytes:?}");
                    assert_eq!(
                        actual_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        trusted_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        "{bytes:?}",
                    );
                    let mut expected_json: serde_json::Value =
                        serde_json::from_slice(bytes).unwrap();
                    if let (Some(selected), Some(object)) =
                        (&selected, expected_json.as_object_mut())
                    {
                        object.retain(|name, _| selected.contains(name));
                    }
                    let expected_value = Value::from_json(expected_json).unwrap();
                    assert_eq!(actual_value, expected_value, "{bytes:?}");
                    let expected_text = if collect_text {
                        scalar_segments(bytes, resolved_text_only, true).unwrap()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(
                        actual_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        expected_text
                            .iter()
                            .map(|segment| (&segment.path, &segment.text))
                            .collect::<Vec<_>>(),
                        "{bytes:?}",
                    );
                }
            }
        }
    }
}

#[test]
fn trusted_projection_does_not_validate_omitted_content_or_trailing_bytes() {
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    let edited = br#"{"wanted":5,"ignored":[1,]} trailing"#;
    assert!(extract_query_record(edited, &projection, false).is_err());
    let (value, text) = extract_query_record(edited, &projection, true).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "wanted".into(),
            Value::Number(Number::integer(5)),
        )])),
    );
    assert!(text.is_empty());
    let escaped = br#"{"wanted":"a\n\"b","ignored":!} trailing"#;
    assert!(extract_query_record(escaped, &projection, false).is_err());
    let (value, _) = extract_query_record(escaped, &projection, true).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "wanted".into(),
            Value::String("a\n\"b".into()),
        )])),
    );
    assert!(extract_query_record(br#"{"wanted":5,"ignored":[1,2}"#, &projection, true).is_err());
    let full_text = ScanProjection::full(true);
    assert!(extract_query_record(br#"{"title":"found"} stale"#, &full_text, false).is_err());
    let (_, text) = extract_query_record(br#"{"title":"found"} stale"#, &full_text, true).unwrap();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].text, "found");
}

#[test]
fn validating_query_extraction_rejects_invalid_json_independently_of_projection() {
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    for bytes in [
        b"{\"wanted\":1,\"ignored\":[1,]}".as_slice(),
        b"{\"wanted\":1,\"ignored\":\"bad\\q\"}".as_slice(),
        b"{\"wanted\":1,\"ignored\":01}".as_slice(),
        b"{\"wanted\":1,\"ignored\":true} extra".as_slice(),
        b"{\"wanted\":1,\"ignored\":\"\xff\"}".as_slice(),
        b"{\"wanted\":1,\"ignored\":\x0b0}".as_slice(),
        b"[[1]".as_slice(),
    ] {
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
        assert!(
            extract_query_record(bytes, &projection, false).is_err(),
            "{bytes:?}"
        );
    }
}

#[test]
fn many_duplicate_members_preserve_all_text_and_erase_replaced_subtrees() {
    let mut source = String::from("{");
    for index in 0..256 {
        if index > 0 {
            source.push(',');
        }
        source.push_str(&format!(
            "\"item\":{{\"number\":{index},\"text\":\"value{index}\"}}"
        ));
    }
    source.push_str(",\"item\":{}}");
    let all = ScanProjection::full(true);
    let (value, text) = extract_query_record(source.as_bytes(), &all, false).unwrap();
    assert_eq!(
        value,
        Value::Object(BTreeMap::from([(
            "item".into(),
            Value::Object(BTreeMap::new()),
        )])),
    );
    assert_eq!(text.len(), 512);
    let resolved = ScanProjection::full_resolved_text();
    let (_, text) = extract_query_record(source.as_bytes(), &resolved, false).unwrap();
    assert!(text.is_empty());
}

#[test]
fn quote_search_handles_escaped_quotes_and_backslash_parity() {
    for slashes in 0..20 {
        let expected = format!("prefix{}\"suffix", "\\".repeat(slashes));
        let bytes = serde_json::to_vec(&expected).unwrap();
        let (value, text) =
            extract_query_record(&bytes, &ScanProjection::full(true), false).unwrap();
        assert_eq!(value, Value::String(expected.clone()));
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].text, expected);
    }
}
