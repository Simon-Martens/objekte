use super::*;

#[test]
fn index_projection_keeps_full_parse_errors_and_precise_selected_values() {
    // Match the stack budget used by Sonic's existing deep-input tests.
    std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let raw = r#"{"score":1,"ignored":{"huge":1e500},"sc\u006fre":9007199254740993}"#;
                assert_eq!(
                    index_field(raw, "score").unwrap()["score"],
                    parse(raw).unwrap()["score"]
                );
                let raw = r#"{"score":1,"title":"first","payload":{"large":[1e500]},"sc\u006fre":9007199254740993,"title":"last"}"#;
                let full = parse(raw).unwrap();
                let selected = index_fields(raw, |name| ["score", "title"].contains(&name)).unwrap();
                assert_eq!(selected, serde_json::json!({"score":full["score"],"title":full["title"]}));
                for raw in [
                    r#"{"score":1,"ignored":"\x"}"#,
                    r#"{"score":1,"ignored":[1,]}"#,
                ] {
                    assert!(parse(raw).is_err());
                    assert!(index_field(raw, "score").is_err());
                }
                for depth in [120, 126, 127, 128, 130] {
                    let raw = format!(
                        "{{\"score\":1,\"ignored\":{}0{}}}",
                        "[".repeat(depth),
                        "]".repeat(depth)
                    );
                    assert_eq!(
                        index_field(&raw, "score").is_ok(),
                        parse(&raw).is_ok(),
                        "{depth}"
                    );
                }
                let raw = r#"[{"score":1},{"score":2}]"#;
                assert_eq!(index_field(raw, "score").unwrap(), parse(raw).unwrap());
            })
            .unwrap()
            .join()
            .unwrap();
}

#[test]
fn full_values_preserve_numbers_and_existing_serialization() {
    for number in [
        "0",
        "-0",
        "1.0",
        "-0.0",
        "1.2300",
        "1e3",
        "1E+03",
        "1e-500",
        "1e500",
        "9007199254740993",
        "-9223372036854775809",
        "18446744073709551616",
        "0.123456789012345678901234567890123456789",
    ] {
        let input = format!(r#"{{"number":{number},"array":[{number},null,true,false]}}"#);
        let expected: Value = serde_json::from_str(&input).unwrap();
        let actual = parse(&input).unwrap();
        assert_eq!(actual, expected, "{number}");
        assert_eq!(actual.to_string(), expected.to_string(), "{number}");
    }
}

#[test]
fn duplicate_and_escaped_keys_keep_the_last_value_at_every_level() {
    let input = r#"{"title":"first","t\u0069tle":"last","payload":{"a":1,"a":2},"array":[{"x":0,"x":3}],"quote\"\\":"é 🐈\n\ud83d\ude00"}"#;
    let expected: Value = serde_json::from_str(input).unwrap();
    assert_eq!(parse(input).unwrap(), expected);
    let projected = project(input, &BTreeSet::from(["title", "payload", "quote\"\\"])).unwrap();
    assert_eq!(projected["title"], "last");
    assert_eq!(projected["payload"], expected["payload"]);
    assert_eq!(projected["quote\"\\"], expected["quote\"\\"]);
    assert!(projected.get("array").is_none());
}

#[test]
fn projected_values_match_full_parse_with_null_missing_and_large_payloads() {
    let input = format!(
        r#"{{"ignored":{{"large":"{}","values":[1e500,{{"a":1}}]}},"title":null,"flag":true,"score":9007199254740993,"title":"last"}}"#,
        "x".repeat(128 * 1024)
    );
    let full: Value = serde_json::from_str(&input).unwrap();
    let fields = BTreeSet::from(["title", "flag", "score", "missing"]);
    let expected: Map<_, _> = full
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, _)| fields.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(project(&input, &fields).unwrap(), Value::Object(expected));
    assert_eq!(
        project(r#"{"title":null}"#, &fields).unwrap()["title"],
        Value::Null
    );
    assert!(project("{}", &fields).unwrap().get("title").is_none());
}

#[test]
fn malformed_json_is_rejected_even_in_unselected_fields() {
    for input in [
        r#"{"title":"ok","ignored":[1,]}"#,
        r#"{"title":"ok","ignored":01}"#,
        r#"{"title":"ok","ignored":"\x"}"#,
        r#"{"title":"ok","ignored":true false}"#,
        r#"{"title":"ok"} true"#,
        r#"{"title":"ok","ignored":{"a":1,}}"#,
    ] {
        assert!(parse(input).is_err(), "full: {input}");
        assert!(
            project(input, &BTreeSet::from(["title"])).is_err(),
            "projected: {input}"
        );
    }
    for input in ["null", "[]", "1", r#""text""#] {
        assert!(project(input, &BTreeSet::new()).is_err());
    }
    assert!(sonic_rs::from_slice::<ParsedValue>(b"{\"bad\":\"\xff\"}").is_err());
}

#[test]
fn projection_does_not_reparse_skipped_surrogate_escapes() {
    let input = r#"{"ignored":{"values":["\ud800","\udc00","\ud83d\ude00"]},"title":"ok"}"#;
    assert_eq!(
        project(input, &BTreeSet::from(["title"])).unwrap(),
        serde_json::json!({"title": "ok"})
    );
}

#[test]
fn full_parse_keeps_the_existing_nesting_limit() {
    // Sonic's own recursion-limit test uses a 16 MiB stack: its unoptimized
    // DOM decoder needs more than Rust's default 2 MiB test-thread stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            for depth in [1, 126, 127, 128, 200] {
                let input = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
                assert_eq!(
                    parse(&input).is_ok(),
                    serde_json::from_str::<Value>(&input).is_ok(),
                    "depth {depth}"
                );
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
