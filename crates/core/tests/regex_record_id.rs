use objekte_search_core::index::bigram::BigramAlphabet;
use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{
    CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions, RegexOptions, parse_filter,
};
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::{FieldDefinition, Schema};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

fn definition() -> CollectionDefinition {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "en".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn regex_id_query(pattern: &str, case: CaseSensitivity, flags: RegexOptions) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: Some(FtsQuery {
            literal: pattern.into(),
            case_sensitivity: case,
            mode: FtsMode::Regex(flags),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: FieldPath::parse("@record.id").unwrap(),
                binding: None,
                query_path: None,
            }],
        }),
        filter: None,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

#[test]
fn selected_regex_uses_record_id_text_without_changing_typed_id_equality() {
    let root = std::env::temp_dir().join(format!(
        "objekte-regex-record-id-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (file, id) in [
        ("a", "AbC-10"),
        ("b", "abc-20"),
        ("c", "Line\nBreak"),
        ("d", "XYZ-30"),
    ] {
        std::fs::write(
            root.join(format!("data/{file}.json")),
            serde_json::json!({"id":id}).to_string(),
        )
        .unwrap();
    }

    let sensitive = regex_id_query(
        r"^AbC-\d+$",
        CaseSensitivity::Sensitive,
        RegexOptions::default(),
    );
    let insensitive = regex_id_query(
        r"^AbC-\d+$",
        CaseSensitivity::Insensitive,
        RegexOptions::default(),
    );
    let cases = [
        (&sensitive, vec!["AbC-10"]),
        (&insensitive, vec!["AbC-10", "abc-20"]),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built = objekte_search_core::lifecycle::Index::start_build(
            &root,
            vec![definition()],
            options,
            objekte_search_core::source::registry::SourceRegistry::builtin(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let built_view = built.view().unwrap();
        let restored = objekte_search_core::lifecycle::Index::start_build(
            &root,
            vec![definition()],
            options,
            SourceRegistry::builtin(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let view = restored.view().unwrap();
        for stored in [false, true] {
            let search = |query: &Query| {
                if stored {
                    view.search(query)
                } else {
                    built_view.search(query)
                }
            };
            for (query, expected) in &cases {
                let actual = search(query).unwrap();
                let reference = built_view.search_reference(query).unwrap();
                let mut ids = actual
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.as_str())
                    .collect::<Vec<_>>();
                ids.sort_unstable();
                assert_eq!(ids, *expected, "{alphabet:?} {query:?}");
                assert_eq!(actual.count, Some(expected.len()));
                assert_eq!(actual.hits, reference.hits);
                assert!(
                    actual
                        .hits
                        .iter()
                        .all(|hit| hit.matched_fields == ["@record.id"])
                );
                assert_eq!(actual.counters.files_opened, 0);
            }
            assert!(search(&sensitive).unwrap().counters.verified_result_hit);

            for (pattern, flags, expected) in [
                ("^Line.Break$", RegexOptions::default(), vec![]),
                (
                    "^Line.Break$",
                    RegexOptions {
                        multi_line: false,
                        dot_matches_new_line: true,
                    },
                    vec!["Line\nBreak"],
                ),
                ("^Break$", RegexOptions::default(), vec![]),
                (
                    "^Break$",
                    RegexOptions {
                        multi_line: true,
                        dot_matches_new_line: false,
                    },
                    vec!["Line\nBreak"],
                ),
            ] {
                let query = regex_id_query(pattern, CaseSensitivity::Sensitive, flags);
                let actual = search(&query).unwrap();
                let reference = built_view.search_reference(&query).unwrap();
                assert_eq!(actual.hits, reference.hits);
                assert_eq!(
                    actual
                        .hits
                        .iter()
                        .map(|hit| hit.record_id.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
            }

            let mut typed_id = insensitive.clone();
            typed_id.filter = Some(parse_filter("id = 'AbC-10'").unwrap());
            assert_eq!(search(&typed_id).unwrap().hits[0].record_id, "AbC-10");
            typed_id.filter = Some(parse_filter("id = 'abc-10'").unwrap());
            assert_eq!(search(&typed_id).unwrap().count, Some(0));
        }
        restored.wait_for_reconciliation().unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
