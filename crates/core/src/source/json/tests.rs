use std::collections::HashSet;

use crate::model::{Number, Value};
use crate::source::json_lines::JsonLinesReader;
use crate::source::registry::SourceRegistry;
use crate::source::{
    DiagnosticCategory, ReaderFailure, ScanProjection, SourceFormat, SourceReader,
    text::visit_logical_strings,
};

use super::document::{parse_json_for_build_diagnosed, parse_json_for_query_diagnosed};
use super::{JsonDecodedScalarsReader, JsonReader};

#[test]
fn schema_free_json_reader_admits_every_root_shape() {
    use crate::schema::Schema;
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
        .register_field(crate::schema::FieldDefinition {
            path: vec!["title".into()],
            kind: crate::model::LogicalType::String,
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
fn root_queries_match_reference_over_mixed_json_documents() {
    use crate::execution_options::ReturnMode;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};

    let root = std::env::temp_dir().join(format!("objekte-json-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        ("array", r#"["first",123,true,null,{"title":"Goethe"}]"#),
        ("string", r#""first""#),
        ("number", "123"),
        ("null", "null"),
        ("object", r#"{"title":"Goethe","values":["first",123]}"#),
        ("quoted", r#"{"@root":"real","a.b":"dot","":"empty"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.collections[0].generation.records.len(), 6);
    for (filter, expected) in [
        ("@root = 'first'", vec!["path:string.json"]),
        ("@root = 123", vec!["path:number.json"]),
        ("@root :null", vec!["path:null.json"]),
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[0] = @root[0]", vec!["path:array.json"]),
        (
            "@root ?= 'first'",
            vec!["path:array.json", "path:string.json"],
        ),
        ("@root[*].title ~ 'Goethe'", vec!["path:array.json"]),
        (
            "@root.title ~ 'Goethe'",
            vec!["path:array.json", "path:object.json"],
        ),
        ("@root[\"a.b\"] = 'dot'", vec!["path:quoted.json"]),
        ("@root[\"\"] = 'empty'", vec!["path:quoted.json"]),
        ("[\"@root\"] = 'real'", vec!["path:quoted.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &crate::query_results::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn declared_schema_keeps_scalar_root_as_diagnosed_fallback() {
    use crate::inventory::RecordClass;
    use crate::model::LogicalType;
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
    use crate::schema::FieldDefinition;

    let root = std::env::temp_dir().join(format!("objekte-declared-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/root.json"), br#"["first", 2]"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let records = &repo.collections[0].generation.records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class, RecordClass::RawFallback);
    assert!(
        repo.files.files[records[0].file.0 as usize]
            .diagnostics
            .iter()
            .any(|issue| issue.message.contains("root must be an object"))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exceptional_numbers_do_not_demote_array_roots_or_dynamic_predicates() {
    use crate::execution_options::ReturnMode;
    use crate::inventory::RecordClass;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::configuration::{
        CollectionDefinition, IdentityPolicy, RepositoryOptions,
    };

    let root =
        std::env::temp_dir().join(format!("objekte-exact-json-numbers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/array.json"),
        b" [ \"first\", 1e1000000, true ] \n",
    )
    .unwrap();
    std::fs::write(root.join("data/object.json"), br#"{"n":-1e-1000000}"#).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", "data", SourceFormat::JsonDecodedScalars);
    definition.identity = IdentityPolicy::RelativePath;
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert!(
        repo.collections[0]
            .generation
            .records
            .iter()
            .all(|record| record.class == RecordClass::Logical)
    );
    for (filter, expected) in [
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[1] = 1e1000000", vec!["path:array.json"]),
        ("n < 0", vec!["path:object.json"]),
        ("n = -1e-1000000", vec!["path:object.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &crate::query_results::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
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
fn trusted_builtin_query_readers_match_validating_results_on_valid_json() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"n":1e0,"array":["\u00df",true,null]}"#;
    let readers: [&dyn SourceReader; 3] =
        [&JsonReader, &JsonDecodedScalarsReader, &JsonLinesReader];
    let projections = [
        ScanProjection::full(true),
        ScanProjection {
            top_level: Some(HashSet::from(["title".into(), "n".into()])),
            collect_text: false,
            resolved_text_only: false,
        },
        ScanProjection {
            top_level: Some(HashSet::from(["title".into()])),
            collect_text: true,
            resolved_text_only: true,
        },
    ];
    for reader in readers {
        for projection in &projections {
            let validating = reader.parse_for_query(bytes, projection).unwrap();
            let trusted = reader.parse_for_query_trusted(bytes, projection).unwrap();
            assert_eq!(validating.value, trusted.value, "{}", reader.name());
            assert_eq!(
                validating
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                trusted
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                "{}",
                reader.name(),
            );
        }
    }
}

#[test]
fn trusted_builtin_projection_omits_search_time_validation_of_skipped_content() {
    let bytes = br#"{"wanted":5,"ignored":[1,]} trailing"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    for reader in [
        &JsonReader as &dyn SourceReader,
        &JsonDecodedScalarsReader,
        &JsonLinesReader,
    ] {
        assert!(
            reader.parse_for_query(bytes, &projection).is_err(),
            "{}",
            reader.name()
        );
        let record = reader.parse_for_query_trusted(bytes, &projection).unwrap();
        let Value::Object(fields) = record.value else {
            panic!("projected JSON root is not an object");
        };
        assert_eq!(
            fields.get("wanted"),
            Some(&Value::Number(Number::integer(5))),
            "{}",
            reader.name(),
        );
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
fn decoded_scalar_fts_matches_tokens_and_preserves_duplicate_name_rules() {
    use crate::execution_options::{BigramAlphabet, ReturnMode};
    use crate::query::{CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions};
    use crate::repository::configuration::{
        CollectionDefinition, IdentityPolicy, RepositoryOptions,
    };

    let root = std::env::temp_dir().join(format!("objekte-scalar-fts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        (
            "one",
            r#"{"needle_only":"x","value":"a\n","num":1.0,"title":"old","title":"new"}"#,
        ),
        ("two", r#"[true,null,1e0,"ß"]"#),
        ("three", r#"{"title":"not here","num":-0}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let cases: &[(&str, Option<&str>, &[&str])] = &[
        ("needle_only", None, &[]),
        ("1.0", None, &["path:one.json"]),
        ("1e0", None, &["path:two.json"]),
        ("-0", None, &["path:three.json"]),
        ("true", None, &["path:two.json"]),
        ("null", None, &["path:two.json"]),
        ("a\n", None, &["path:one.json"]),
        ("old", None, &["path:one.json"]),
        ("old", Some("title"), &[]),
        ("new", Some("title"), &["path:one.json"]),
        ("1e0", Some("@root"), &["path:two.json"]),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut definition =
            CollectionDefinition::folder("data", "data", SourceFormat::JsonDecodedScalars);
        definition.identity = IdentityPolicy::RelativePath;
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                ..Default::default()
            },
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        for &(literal, selected, expected) in cases {
            let query = Query {
                collections: vec!["data".into()],
                fts: Some(FtsQuery {
                    case_sensitivity: CaseSensitivity::Sensitive,
                    mode: FtsMode::Literal,
                    literal: literal.into(),
                    pattern_parameter: None,
                    fields: selected
                        .into_iter()
                        .map(|field| FtsField {
                            path: crate::model::FieldPath::parse(field).unwrap(),
                            binding: None,
                            query_path: None,
                        })
                        .collect(),
                }),
                filter: None,
                options: QueryOptions {
                    sort: vec![],
                    limit: 20,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let optimized = repo.search(&query).unwrap();
            let ids = |result: &crate::query_results::SearchResult| {
                result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                ids(&optimized),
                expected,
                "{literal:?} {selected:?} {alphabet:?}"
            );
            if selected.is_none() {
                let reference = repo.search_reference(&query).unwrap();
                assert_eq!(
                    ids(&optimized),
                    ids(&reference),
                    "{literal:?} {selected:?} {alphabet:?}"
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_json_borrows_logical_strings_after_temporal_conversion() {
    use crate::execution_options::BigramAlphabet;
    use crate::index::bigram::BigramBuilder;
    use crate::model::LogicalType;
    use crate::record_preparation::finish_build_record;
    use crate::schema::{FieldDefinition, Schema};

    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let bytes =
        r#"{"date":"2026-01-02","title":"aßb","nested":{"names":["ab","cd",""]}}"#.as_bytes();
    let mut record = source.reader().parse_for_build_outcome(bytes).unwrap();
    assert!(record.text.is_empty());
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["date".into()],
            kind: LogicalType::Date,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    finish_build_record(&schema, &mut record).unwrap();
    let mut strings = Vec::new();
    visit_logical_strings(&record.value, &mut |text| strings.push(text.to_owned()));
    assert_eq!(strings, ["ab", "cd", "", "aßb"]);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut borrowed = BigramBuilder::new(alphabet);
        borrowed
            .add_record_with(crate::model::RecordSlot(0), |emit| {
                visit_logical_strings(&record.value, emit);
            })
            .unwrap();
        let borrowed = borrowed.finish().unwrap();
        let mut owned = BigramBuilder::new(alphabet);
        owned
            .add_record(crate::model::RecordSlot(0), strings.iter())
            .unwrap();
        let owned = owned.finish().unwrap();
        assert_eq!(borrowed.normal, owned.normal);
        assert_eq!(borrowed.skip, owned.skip);
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

#[test]
fn query_projection_skips_unneeded_fields_and_preserves_selected_values() {
    let bytes = br#"{"ignored":{"deep":[41,42]},"needed":{"name":"found"},"other":"hidden"}"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["needed".to_owned()])),
        collect_text: true,
        resolved_text_only: false,
    };
    for format in [SourceFormat::Json, SourceFormat::JsonLines] {
        let source = SourceRegistry::builtin().resolve(&format).unwrap();
        let selected = source.reader().parse_for_query(bytes, &projection).unwrap();
        assert_eq!(
            selected.value,
            Value::from_json(serde_json::json!({"needed":{"name":"found"}})).unwrap()
        );
        assert_eq!(selected.text.len(), 1);
        assert_eq!(selected.text[0].path, ["needed", "name"]);
        assert_eq!(selected.text[0].text, "found");
        let full = source
            .reader()
            .parse_for_query(bytes, &ScanProjection::full(false))
            .unwrap();
        assert!(matches!(full.value, Value::Object(ref fields) if fields.len() == 3));
        assert!(full.text.is_empty());
        assert!(
            source
                .reader()
                .parse_for_query(br#"{"needed":"ok","ignored":[1,]}"#, &projection)
                .is_err()
        );
    }
}

#[test]
fn standalone_jsonl_ranges_preserve_bytes_and_boundaries() {
    let bytes = "ü\n{\"x\":\"ä\"}\n\n{\"x\":\"b\"}\n".as_bytes();
    // A non-ASCII prefix is deliberately outside the valid records.
    let start = "ü\n".len();
    let valid = &bytes[start..];
    let ranges = JsonLinesReader.frame(valid).unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(
        &valid[ranges[0].start as usize..ranges[0].end as usize],
        "{\"x\":\"ä\"}".as_bytes()
    );
    for range in ranges {
        JsonLinesReader
            .parse(&valid[range.start as usize..range.end as usize])
            .unwrap();
    }
    assert!(JsonLinesReader.frame(b"").unwrap().is_empty());
}
