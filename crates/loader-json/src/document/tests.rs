use objekte_search_core::source::{
    DiagnosticCategory, ReaderFailure, ScanProjection, SourceReader,
};

use super::document::{parse_json_for_build_diagnosed, parse_json_for_query_diagnosed};
use super::{JsonDecodedScalarsReader, JsonReader};

#[test]
fn schema_free_json_reader_admits_every_root_shape() {
    use objekte_search_core::schema::Schema;
    let reader = JsonReader;
    for text in [
        r#"{}"#,
        r#"[]"#,
        r#"["a",1,true,null,{}]"#,
        r#""root""#,
        "42",
        "true",
        "false",
        "null",
    ] {
        let mut record = reader.parse_outcome(text.as_bytes()).unwrap();
        Schema::default().coerce_record(&mut record.value).unwrap();
    }
    let mut declared = Schema::default();
    declared
        .register_field(objekte_search_core::schema::FieldDefinition {
            path: vec!["title".into()],
            kind: objekte_search_core::model::LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let mut scalar = reader.parse_outcome(b"42").unwrap();
    assert!(declared.coerce_record(&mut scalar.value).is_err());
}

#[test]
fn direct_build_json_matches_existing_logical_reader_and_diagnostics() {
    let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let cases: Vec<&[u8]> = vec![
        br#"null"#,
        br#"[]"#,
        br#"{}"#,
        br#"[null,true,0,-0,1.0,1e2,"x",{"a":[]}]"#,
        br#"{"a":"escaped\n\"quote","a":"last","emoji":"\uD83D\uDE03"}"#,
        br#"{"n":170141183460469231731687303715884105727}"#,
        br#"{"n":170141183460469231731687303715884105728}"#,
        br#"{"n":18446744073709551615,"negative":-9223372036854775809}"#,
        br#"{"n":1.234567890123456789e-300}"#,
        br#"{"n":0.123456789012345678901234567890123456789}"#,
        br#"{"n":1e400}"#,
        br#"{"$serde_json::private::Number":"17"}"#,
        br#"{"$serde_json::private::Number":"17","other":1}"#,
        br#"{"other":1,"$serde_json::private::Number":"17"}"#,
        br#"{"$serde_json::private::Number":17}"#,
        br#"{"nested":[[null],[1,"x"],[]]}"#,
        br#"{"broken":,}"#,
        br#"{"broken":"\uD800"}"#,
        br#"{"broken":"#,
        b"{\"broken\":\"\xff\"}",
        deep.as_bytes(),
    ];
    for bytes in cases {
        let expected = parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false));
        let actual = parse_json_for_build_diagnosed(bytes);
        match (actual, expected) {
            (Ok(actual), Ok(expected)) => {
                assert_eq!(
                    actual.value,
                    expected.value,
                    "{}",
                    String::from_utf8_lossy(bytes)
                );
                assert!(actual.text.is_empty());
            }
            (Err(actual), Err(expected)) => assert_eq!(actual, expected),
            (actual, expected) => panic!("direct/legacy mismatch: {actual:?} vs {expected:?}"),
        }
    }
    let decoded = JsonDecodedScalarsReader
        .parse_for_build_outcome(br#"{"a":1,"a":2}"#)
        .unwrap();
    let ordinary = JsonReader
        .parse_for_build_outcome(br#"{"a":1,"a":2}"#)
        .unwrap();
    assert_ne!(decoded.text.len(), ordinary.text.len());
}

#[test]
fn valid_json_beyond_numeric_resource_limit_is_fatal_not_malformed_fallback() {
    let reader = JsonDecodedScalarsReader;
    let result = reader.parse_outcome(br#"{"n":1e99999999999999999999}"#);
    assert!(
        matches!(result, Err(ReaderFailure::Fatal(message)) if message.contains("resource limit"))
    );
}

#[test]
fn syntactically_valid_deep_json_reports_resource_limit() {
    let reader = JsonDecodedScalarsReader;
    let nested = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    let result = reader.parse_outcome(nested.as_bytes());
    assert!(
        matches!(result, Err(ReaderFailure::Fatal(message)) if message.contains("recursion limit"))
    );
}

#[test]
fn decoded_scalar_build_and_query_accept_the_same_json_documents() {
    let reader = JsonDecodedScalarsReader;
    let mut documents: Vec<Vec<u8>> = vec![
        br#"{"title":"first","title":"last","values":[null,true,1,1.0,1e0,-0,"\u00df","\ud83d\ude80"]}"#.to_vec(),
        br#"["first",123,false,null,{"nested":"\uD834\uDD1E"}]"#.to_vec(),
        br#""a root string""#.to_vec(),
        b"1234567890123456789012345678901234567890".to_vec(),
        b"true".to_vec(),
        b"false".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"[1,]".to_vec(),
        b"{\"a\":1,}".to_vec(),
        b"01".to_vec(),
        b"1e".to_vec(),
        b"1e+".to_vec(),
        b"-".to_vec(),
        b"\"bad\\q\"".to_vec(),
        b"\"\\ud800\"".to_vec(),
        b"\"\\udc00\"".to_vec(),
    ];
    for depth in [126, 127, 128, 129, 130] {
        for (open, close) in [("[", "]"), ("{\"a\":", "}")] {
            let bytes = format!("{}0{}", open.repeat(depth), close.repeat(depth));
            let built = reader.parse_for_build_outcome(bytes.as_bytes());
            let queried = reader.parse_outcome(bytes.as_bytes());
            assert_eq!(
                built.is_ok(),
                queried.is_ok(),
                "nesting depth {depth}: {open}"
            );
        }
    }
    for control in 0..=31 {
        documents.push(vec![b'"', b'a', control, b'b', b'"']);
    }
    documents.push(vec![b'"', 0xff, b'"']);
    documents.push(b"\xef\xbb\xbf{}".to_vec());
    for bytes in documents {
        let built = reader.parse_for_build_outcome(&bytes);
        let queried = reader.parse_outcome(&bytes);
        assert_eq!(
            built.is_ok(),
            queried.is_ok(),
            "fresh/query acceptance differs for {bytes:?}: build={built:?}, query={queried:?}"
        );
        if let (Ok(built), Ok(queried)) = (built, queried) {
            assert_eq!(built.value, queried.value, "{bytes:?}");
            assert_eq!(
                built
                    .text
                    .iter()
                    .map(|segment| &segment.text)
                    .collect::<Vec<_>>(),
                queried
                    .text
                    .iter()
                    .map(|segment| &segment.text)
                    .collect::<Vec<_>>(),
                "{bytes:?}"
            );
        }
    }
}

#[test]
fn decoded_scalar_build_uses_exact_json_whitespace() {
    let reader = JsonDecodedScalarsReader;
    for control in (0..=31).chain([b' ', 127]) {
        for (prefix, suffix) in [
            (b"".as_slice(), b"0".as_slice()),
            (b"0".as_slice(), b"".as_slice()),
            (b"[".as_slice(), b"0]".as_slice()),
            (b"[0".as_slice(), b",1]".as_slice()),
            (b"{\"a\":".as_slice(), b"0}".as_slice()),
            (b"{\"a\":0".as_slice(), b"}".as_slice()),
        ] {
            let mut bytes = prefix.to_vec();
            bytes.push(control);
            bytes.extend_from_slice(suffix);
            let built = reader.parse_for_build_outcome(&bytes);
            let queried = reader.parse_outcome(&bytes);
            let allowed = matches!(control, b' ' | b'\t' | b'\n' | b'\r');
            assert_eq!(
                queried.is_ok(),
                allowed,
                "query control {control:#04x} in {bytes:?}"
            );
            assert_eq!(
                built.is_ok(),
                allowed,
                "build control {control:#04x} in {bytes:?}"
            );
        }
    }
}

#[test]
fn json_reader_reports_syntax_and_encoding_positions() {
    let reader = JsonReader;
    let syntax = reader.parse_outcome(b"{\n\"title\":,}").unwrap_err();
    let ReaderFailure::Malformed(syntax) = syntax else {
        panic!("malformed syntax")
    };
    assert_eq!(syntax.category, DiagnosticCategory::Syntax);
    assert_eq!(syntax.code, "json.parse");
    assert_eq!(syntax.line, Some(2));
    assert!(syntax.column.is_some());
    assert!(syntax.relative_byte.is_some_and(|offset| offset < 13));
    let encoding = reader.parse_outcome(b"{\"title\":\"\xff\"}").unwrap_err();
    let ReaderFailure::Malformed(encoding) = encoding else {
        panic!("malformed encoding")
    };
    assert_eq!(encoding.category, DiagnosticCategory::Encoding);
    assert_eq!(encoding.relative_byte, Some(10));
}
