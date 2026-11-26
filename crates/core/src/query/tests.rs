use super::test_support::{QueryFixture, query_from_fixture};
use super::*;

#[test]
fn parses_boolean_state_modifiers_without_changing_question_operators_or_literals() {
    for (source, modifier) in [
        ("a:? = true", Modifier::NonEmpty),
        ("a:string = true", Modifier::StringType),
        ("a:number = false", Modifier::NumberType),
        ("a:array != true", Modifier::ArrayType),
        ("a:null = true", Modifier::Null),
        ("a:object = true", Modifier::ObjectType),
        ("a:exists = true", Modifier::Exists),
    ] {
        let FilterExpr::Term(term) = parse_filter(source).unwrap() else {
            panic!("term")
        };
        assert_eq!(term.modifiers, vec![modifier], "{source}");
        assert!(matches!(term.operand, Operand::Literal(Literal::Bool(_))));
    }
    let FilterExpr::Term(term) = parse_filter("a ?= 'what?'").unwrap() else {
        panic!("term")
    };
    assert_eq!(term.quantifier, Quantifier::Any);
    assert_eq!(
        term.operand,
        Operand::Literal(Literal::String("what?".into()))
    );
    let FilterExpr::Term(term) = parse_filter("data[\"a?b\"]:? = true").unwrap() else {
        panic!("term")
    };
    assert_eq!(term.modifiers, vec![Modifier::NonEmpty]);
}

#[test]
fn sort_span_ignores_literals_and_nested_text() {
    let text = r#"title = 'SORT BY year' && note = 'x' SORT BY year DESC"#;
    let start = structured_sort_start(text).unwrap().unwrap();
    assert_eq!(&text[start..], "SORT BY year DESC");
    assert_eq!(structured_sort_start("SORT BY year ASC").unwrap(), Some(0));
    assert_eq!(structured_sort_start("title = 'SORT BY'").unwrap(), None);
    assert!(structured_sort_start("title = ").is_err());
}

#[test]
fn sort_grammar_preserves_typed_reductions_positions_and_bindings() {
    let parsed = parse_structured_filter(
        "SORT BY min(prices[*]) DESC, contributors[0].name, body:normalized ASC, LAST(@backlinks.mentions.year)",
    )
    .unwrap();
    let fields = parsed.sort.unwrap();
    assert_eq!(fields.len(), 4);
    assert_eq!(
        fields[0].expression.as_ref().unwrap().reducer,
        Some(SortReducer::Min)
    );
    assert_eq!(fields[0].expression.as_ref().unwrap().path, "prices[*]");
    assert!(fields[0].descending);
    assert_eq!(
        fields[1].expression.as_ref().unwrap().path,
        "contributors[0].name"
    );
    assert_eq!(
        fields[2].expression.as_ref().unwrap().binding.as_deref(),
        Some("normalized")
    );
    assert_eq!(
        fields[3].expression.as_ref().unwrap().reducer,
        Some(SortReducer::Last)
    );
    assert!(parse_structured_filter("SORT BY MIN(title").is_err());
    assert!(parse_structured_filter("SORT BY MIN(title), MIN(title)").is_err());
}

#[test]
fn precedence_quantifiers_and_separate_fts() {
    let ast = parse_filter("a = 1 || b = 2 && !(c ?!= 'x')").unwrap();
    assert!(matches!(ast, FilterExpr::Or(parts) if parts.len() == 2));
    let ast = parse_filter("data.gnd[\"@context\"] ?= 'x' && @file.size >= 1").unwrap();
    assert!(matches!(ast, FilterExpr::And(parts) if parts.len() == 2));
    assert!(parse_filter("a => 1").is_err());
    assert!(parse_filter("a =< 1").is_err());
    assert!(parse_filter("a ?=! 1").is_err());
    let case = QueryFixture {
        fts_mode: FtsMode::Literal,
        fts_case_sensitivity: crate::query::CaseSensitivity::Sensitive,
        collection: "*".into(),
        fts: Some("a && b".into()),
        filter: Some("a = 1".into()),
        fts_fields: vec![],
        sort: vec![],
        limit: 50,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    let query = query_from_fixture(&case).unwrap();
    assert_eq!(query.fts.unwrap().literal, "a && b");
    let mut regex_case = case.clone();
    regex_case.fts_mode = FtsMode::Regex(RegexOptions {
        multi_line: true,
        dot_matches_new_line: false,
    });
    assert_eq!(
        query_from_fixture(&regex_case).unwrap().fts.unwrap().mode,
        regex_case.fts_mode
    );
    let mut insensitive = case.clone();
    insensitive.fts_case_sensitivity = CaseSensitivity::Insensitive;
    assert_eq!(
        query_from_fixture(&insensitive)
            .unwrap()
            .fts
            .unwrap()
            .case_sensitivity,
        CaseSensitivity::Insensitive
    );
}

#[test]
fn workload_fts_binding_colon_stays_outside_quoted_path_properties() {
    let case = QueryFixture {
        collection: "data".into(),
        fts: Some("needle".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: None,
        fts_fields: vec![
            "[\"a:b\"]".into(),
            "['a:b']".into(),
            "items[0][\"a:b\"]:lower".into(),
            "x:lower".into(),
            "[\"a b\"]".into(),
            "[\"@root\"]".into(),
            "[\"\"]".into(),
            "items['0']".into(),
            "items[*].name".into(),
        ],
        sort: vec![],
        limit: 10,
        offset: 0,
        count: false,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    let fields = query_from_fixture(&case).unwrap().fts.unwrap().fields;
    assert_eq!(fields[0].path.segments, ["a:b"]);
    assert_eq!(fields[0].binding, None);
    assert_eq!(fields[1].path.segments, ["a:b"]);
    assert_eq!(fields[1].binding, None);
    assert_eq!(fields[2].path.segments, ["items", "a:b"]);
    assert_eq!(fields[2].binding.as_deref(), Some("lower"));
    assert_eq!(fields[2].query_path.as_deref(), Some("items[0][\"a:b\"]"));
    assert_eq!(fields[3].path.segments, ["x"]);
    assert_eq!(fields[3].binding.as_deref(), Some("lower"));
    assert_eq!(fields[4].path.segments, ["a b"]);
    assert_eq!(fields[5].path.segments, ["@root"]);
    assert_eq!(fields[6].path.segments, [""]);
    assert_eq!(fields[7].path.segments, ["items", "0"]);
    assert_eq!(fields[8].path.segments, ["items", "name"]);
    assert_eq!(fields[8].query_path.as_deref(), Some("items[*].name"));
}

#[test]
fn quoted_colon_field_has_one_schema_filter_fts_and_sort_address() {
    use crate::model::{FieldPath, LogicalType};
    use crate::repository::Repository;
    use crate::repository::configuration::RepositoryOptions;
    use crate::schema::FieldDefinition;

    let root = std::env::temp_dir().join(format!(
        "objekte-colon-path-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.row"), r#"("a:b"="Zulu")"#).unwrap();
    std::fs::write(root.join("data/two.row"), r#"("a:b"="Alpha")"#).unwrap();
    let mut definition = crate::source::test_support::document_folder("data", "data");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["a:b".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition.sort_fields = vec![FieldPath::parse("[\"a:b\"]").unwrap()];
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let mut case = QueryFixture {
        collection: "data".into(),
        fts: None,
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: Some("[\"a:b\"] = 'Alpha'".into()),
        fts_fields: vec![],
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    let ids = |repo: &Repository, query: &Query| {
        let optimized = repo.search(query).unwrap();
        assert_eq!(optimized.hits, repo.search_reference(query).unwrap().hits);
        optimized
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&repo, &query_from_fixture(&case).unwrap()),
        ["path:two.row"]
    );

    case.filter = None;
    case.fts = Some("Alpha".into());
    case.fts_fields = vec!["[\"a:b\"]".into()];
    assert_eq!(
        ids(&repo, &query_from_fixture(&case).unwrap()),
        ["path:two.row"]
    );

    case.fts = None;
    case.fts_fields.clear();
    case.filter = Some("SORT BY [\"a:b\"]".into());
    assert_eq!(
        ids(&repo, &query_from_fixture(&case).unwrap()),
        ["path:two.row", "path:one.row"]
    );
    drop(repo);
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(
        ids(&restored, &query_from_fixture(&case).unwrap()),
        ["path:two.row", "path:one.row"]
    );
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn seeded_bounded_filter_inputs_never_panic() {
    const SEED: u64 = 0xF11E_2026_1106;
    let mut state = SEED;
    let alphabet = [
        'a', '0', ' ', '(', ')', '[', ']', '\'', '"', ':', '?', '=', '!', '&', '|', '~', '.', '\\',
        'é',
    ];
    for case in 0..512 {
        let len = (state as usize % 48) + 1;
        let mut input = String::new();
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            input.push(alphabet[state as usize % alphabet.len()]);
        }
        assert!(
            std::panic::catch_unwind(|| parse_filter(&input)).is_ok(),
            "seed={SEED:#x} case={case} input={input:?}"
        );
    }
}

#[test]
fn typed_operand_shapes_preserve_source_spans() {
    let FilterExpr::Term(term) = parse_filter("title:lower = alternate_title:lower").unwrap()
    else {
        panic!("expected term");
    };
    assert_eq!(term.span, (0, 35));
    let Operand::Field(field) = term.operand else {
        panic!("expected field operand")
    };
    assert_eq!(field.path.field.segments, ["alternate_title"]);
    assert_eq!(field.modifiers, [Modifier::Lower]);
    assert_eq!(field.span, (14, 35));

    let FilterExpr::Term(term) = parse_filter("year >= $minimum_year").unwrap() else {
        panic!("expected term");
    };
    assert!(
        matches!(term.operand, Operand::Parameter { name, bound: None } if name == "minimum_year")
    );
    assert_eq!(term.span, (0, 21));
    assert!(parse_filter("year >= $").is_err());
    assert!(parse_filter("year >= $2x").is_err());

    let FilterExpr::Term(term) = parse_filter("title = 'alternate_title'").unwrap() else {
        panic!("expected term");
    };
    assert!(
        matches!(term.operand, Operand::Literal(Literal::String(value)) if value == "alternate_title")
    );
}

#[test]
fn inline_sort_uses_only_top_level_keywords() {
    let parsed =
        parse_structured_filter("['sort'].value = 'SORT BY' SORT by preferred_title desc, year")
            .unwrap();
    assert!(parsed.filter.is_some());
    assert_eq!(
        parsed.sort,
        Some(vec![
            SortField {
                field: "preferred_title".into(),
                descending: true,
                expression: None,
            },
            SortField {
                field: "year".into(),
                descending: false,
                expression: None,
            },
        ])
    );
    assert!(
        parse_structured_filter("sort = 1")
            .unwrap()
            .filter
            .is_some()
    );
    assert!(parse_structured_filter("sort = 1").unwrap().sort.is_none());
    assert_eq!(
        parse_structured_filter("SORT BY year DESC").unwrap().filter,
        None
    );
    assert!(parse_filter("title = 'SORT BY'").is_ok());
    assert!(parse_filter("year >= 1 SORT BY year").is_err());
    assert!(parse_structured_filter("(year >= 1 SORT BY year)").is_err());
}

#[test]
fn inline_sort_rejects_bad_or_conflicting_clauses() {
    for input in [
        "SORT BY",
        "SORT BY year,",
        "SORT BY year UP",
        "SORT BY year DESC trailing",
        "SORT BY year, year DESC",
        "SORT BY year SORT BY title",
        "year >= 1 SORT BY year, year",
        "SORT BY $parameter",
    ] {
        assert!(parse_structured_filter(input).is_err(), "{input}");
    }
    for input in [
        "SORT BY items[*].price",
        "SORT BY items[0].price",
        "SORT BY matrix[1][2]",
    ] {
        assert!(parse_structured_filter(input).is_ok(), "{input}");
    }
    let mut case = QueryFixture {
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        collection: "*".into(),
        fts: Some("Hymn".into()),
        filter: Some("SORT BY year DESC".into()),
        fts_fields: vec![],
        sort: vec![],
        limit: 3,
        offset: 2,
        count: true,
        return_mode: ReturnMode::Logical,
        evidence: true,
    };
    let query = query_from_fixture(&case).unwrap();
    assert!(query.filter.is_none());
    assert!(query.fts.is_some());
    assert_eq!(
        query.options.sort,
        vec![SortField {
            field: "year".into(),
            descending: true,
            expression: None,
        }]
    );
    assert_eq!(query.options.limit, 3);
    assert_eq!(query.options.offset, 2);
    case.sort.push(SortField {
        field: "year".into(),
        descending: true,
        expression: None,
    });
    assert!(query_from_fixture(&case).unwrap_err().contains("conflicts"));
}

#[test]
fn complete_regex_pattern_parameter_binds_once_without_interpolation() {
    let mut case = QueryFixture {
        collection: "data".into(),
        fts: Some("$pattern".into()),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Regex(RegexOptions::default()),
        filter: None,
        fts_fields: vec![],
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    let template = query_from_fixture(&case).unwrap();
    assert_eq!(
        template.fts.as_ref().unwrap().pattern_parameter.as_deref(),
        Some("pattern")
    );
    assert!(
        template
            .with_parameters(&std::collections::HashMap::new())
            .is_err()
    );
    let bound = template
        .with_parameters(&std::collections::HashMap::from([(
            "pattern".into(),
            Value::String("$other|^Hymn$".into()),
        )]))
        .unwrap();
    assert_eq!(bound.fts.as_ref().unwrap().literal, "$other|^Hymn$");
    assert!(bound.fts.as_ref().unwrap().pattern_parameter.is_none());
    case.fts_mode = FtsMode::Literal;
    assert!(
        query_from_fixture(&case)
            .unwrap()
            .fts
            .unwrap()
            .pattern_parameter
            .is_none()
    );
    case.fts_mode = FtsMode::Regex(RegexOptions::default());
    case.fts = Some("prefix$pattern".into());
    assert!(
        query_from_fixture(&case)
            .unwrap()
            .fts
            .unwrap()
            .pattern_parameter
            .is_none()
    );
}

// Expand each suite in its original module so it can access private items.
macro_rules! clock_tests {
    () => {
        use super::*;
        use time::format_description::well_known::Rfc3339;

        fn frozen() -> ClockContext {
            ClockContext::new(
                OffsetDateTime::parse("2026-11-08T23:59:59.5+02:00", &Rfc3339).unwrap(),
            )
        }

        fn period(operand: Operand) -> PeriodLiteral {
            let Operand::Period(period) = operand else {
                panic!("expected a temporal period")
            };
            period
        }

        fn integer(operand: Operand) -> i128 {
            let Operand::Literal(Literal::Number(Number::Integer(number))) = operand else {
                panic!("expected a numeric component")
            };
            number
        }

        #[test]
        fn frozen_clock_resolves_utc_components_and_periods() {
            let clock = frozen();
            // The explicit +02:00 input is Sunday locally but Sunday 21:59 UTC too.
            assert_eq!(integer(clock.resolve("second").unwrap()), 59);
            assert_eq!(integer(clock.resolve("minute").unwrap()), 59);
            assert_eq!(integer(clock.resolve("hour").unwrap()), 21);
            assert_eq!(integer(clock.resolve("weekday").unwrap()), 0);
            assert_eq!(integer(clock.resolve("day").unwrap()), 8);
            assert_eq!(integer(clock.resolve("month").unwrap()), 11);
            assert_eq!(integer(clock.resolve("year").unwrap()), 2026);

            let now = period(clock.resolve("now").unwrap());
            assert_eq!(now.end_ns - now.start_ns, 1);
            assert_eq!(
                period(clock.resolve("yesterday").unwrap()).start_ns,
                now.start_ns - DAY_NS
            );
            assert_eq!(
                period(clock.resolve("tomorrow").unwrap()).start_ns,
                now.start_ns + DAY_NS
            );
            let today = period(clock.resolve("today").unwrap());
            assert_eq!(today.end_ns - today.start_ns, DAY_NS);
            assert_eq!(
                period(clock.resolve("todayStart").unwrap()).start_ns,
                today.start_ns
            );
            assert_eq!(
                period(clock.resolve("todayEnd").unwrap()).start_ns,
                today.end_ns - 1
            );
            let month = period(clock.resolve("thisMonth").unwrap());
            assert_eq!(
                period(clock.resolve("monthStart").unwrap()).start_ns,
                month.start_ns
            );
            assert_eq!(
                period(clock.resolve("monthEnd").unwrap()).start_ns,
                month.end_ns - 1
            );
            let year = period(clock.resolve("thisYear").unwrap());
            assert_eq!(
                period(clock.resolve("yearStart").unwrap()).start_ns,
                year.start_ns
            );
            assert_eq!(
                period(clock.resolve("yearEnd").unwrap()).start_ns,
                year.end_ns - 1
            );
            assert!(
                clock
                    .resolve("unknown")
                    .unwrap_err()
                    .contains("unknown time macro")
            );
        }

        #[test]
        fn unrelated_calendar_overflow_does_not_reject_instant_or_components() {
            let clock = ClockContext::new(Date::MAX.midnight().assume_utc());
            assert!(clock.resolve("now").is_ok());
            assert!(clock.resolve("second").is_ok());
            assert!(clock.resolve("todayStart").is_ok());
            assert!(clock.resolve("monthStart").is_ok());
            assert!(clock.resolve("yearStart").is_ok());
            assert!(clock.resolve("todayEnd").is_err());
        }
    };
}
pub(super) use clock_tests;

macro_rules! completion_tests {
    () => {
        use super::*;

        #[test]
        fn detects_partial_field_and_value_without_valid_whole_expression() {
            assert_eq!(
                context("na >=", 2).unwrap().target,
                CompletionTarget::Field { sort: false }
            );
            assert_eq!(
                context("year >= 19", 10).unwrap().target,
                CompletionTarget::Value {
                    field: "year".into()
                }
            );
            assert_eq!(context("year >= \"Hei", 12).unwrap().replace, 8..12);
        }

        #[test]
        fn completes_boolean_field_tests_without_treating_quoted_colons_as_modifiers() {
            let modifier = context("annotation:st", "annotation:st".len()).unwrap();
            assert_eq!(modifier.target, CompletionTarget::Modifier);
            assert_eq!(modifier.replace, 11..13);
            assert_eq!(modifier.prefix, "st");
            assert_eq!(
                insert("annotation:st", modifier.replace, "string")
                    .unwrap()
                    .0,
                "annotation:string"
            );
            assert_eq!(
                context("annotation:? = tr", "annotation:? = tr".len())
                    .unwrap()
                    .target,
                CompletionTarget::BooleanValue
            );
            assert_eq!(
                context("[\"a:b\"]:nu", "[\"a:b\"]:nu".len())
                    .unwrap()
                    .target,
                CompletionTarget::Modifier
            );
            assert_eq!(
                context("[\"a:b\"] = tr", "[\"a:b\"] = tr".len())
                    .unwrap()
                    .target,
                CompletionTarget::Value {
                    field: "[\"a:b\"]".into()
                }
            );
        }

        #[test]
        fn insertion_keeps_surrounding_expression_and_unicode_offsets() {
            let text = "name = \"Häi\" && age > 1";
            let position = text.find("Häi").unwrap() + "Hä".len();
            let completion = context(text, position).unwrap();
            assert_eq!(
                completion.target,
                CompletionTarget::Value {
                    field: "name".into()
                }
            );
            let (result, _) = insert(text, completion.replace, "\"Heidelberg\"").unwrap();
            assert_eq!(result, "name = \"Heidelberg\" && age > 1");
        }

        #[test]
        fn quoted_nested_path_replaces_one_complete_token() {
            let text = "foo[\"ba\"] = 1";
            let at = text.find("ba").unwrap() + 2;
            let context = context(text, at).unwrap();
            assert_eq!(context.replace, 0..9);
            assert_eq!(context.prefix, "foo[\"ba");
            assert_eq!(context.target, CompletionTarget::Field { sort: false });
            assert_eq!(
                insert(text, context.replace, "foo[\"bar\"]").unwrap().0,
                "foo[\"bar\"] = 1"
            );
        }

        #[test]
        fn offers_fields_only_where_a_field_can_begin() {
            for text in [
                "name ",
                "name:lower ",
                "name = 'Ada' ",
                "name = 'Ada' SORT ",
                "!",
                "name = 'Ada' !",
                "name = 'Ada' (",
                "name = 'Ada' && !",
            ] {
                assert!(context(text, text.len()).is_none(), "{text}");
            }
            for text in [
                "",
                "(",
                "! (",
                "name = 'Ada' && ",
                "name = 'Ada' || (",
                "name = 'Ada' && !( ",
            ] {
                assert_eq!(
                    context(text, text.len()).unwrap().target,
                    CompletionTarget::Field { sort: false },
                    "{text}"
                );
            }
        }

        #[test]
        fn sort_completion_stops_after_a_field_or_direction() {
            for text in [
                "SORT BY ",
                "SORT  BY ",
                "name = 'Ada' SORT BY title, ",
                "SORT BY MIN(",
                "SORT BY MIN (",
                "SORT BY title, MAX (",
            ] {
                assert_eq!(
                    context(text, text.len()).unwrap().target,
                    CompletionTarget::Field { sort: true },
                    "{text}"
                );
            }
            for text in ["SORT BY title ", "SORT BY title ASC", "SORT BY MIN(title) "] {
                assert!(context(text, text.len()).is_none(), "{text}");
            }
            let quoted = "name = 'SORT BY' && title";
            assert_eq!(
                context(quoted, quoted.len()).unwrap().target,
                CompletionTarget::Field { sort: false }
            );
        }

        #[test]
        fn single_quoted_values_and_spaced_nested_paths_keep_their_token() {
            let value = "name ~ 'A%'";
            let caret = value.find('%').unwrap() + 1;
            let found = context(value, caret).unwrap();
            assert_eq!(found.replace, 7..11);
            assert_eq!(found.prefix, "A%");
            assert_eq!(
                found.target,
                CompletionTarget::Value {
                    field: "name".into()
                }
            );

            let field = "foo['a b'] = 1";
            let caret = field.find('b').unwrap() + 1;
            let found = context(field, caret).unwrap();
            assert_eq!(found.replace, 0..10);
            assert_eq!(found.target, CompletionTarget::Field { sort: false });

            let nested_value = "name = 'x' && foo['a b'] ~ 'Al";
            let found = context(nested_value, nested_value.len()).unwrap();
            assert_eq!(
                found.target,
                CompletionTarget::Value {
                    field: "foo['a b']".into()
                }
            );
        }

        #[test]
        fn standalone_and_finished_strings_do_not_offer_fields() {
            assert!(context("\"Alpha\"", 1).is_none());
            assert!(context("name = 'Ada'", "name = 'Ada'".len()).is_none());
            assert!(context("SORT BY 'x", "SORT BY 'x".len()).is_none());
        }

        #[test]
        fn metadata_paths_complete_in_filters_and_sorts() {
            for prefix in ["@record.", "@file.", "@collection."] {
                for before in ["", "year > 0 && ", "SORT BY "] {
                    let text = format!("{before}{prefix}");
                    let found = context(&text, text.len()).unwrap();
                    assert_eq!(found.prefix, prefix);
                    assert_eq!(
                        found.target,
                        CompletionTarget::Field {
                            sort: before == "SORT BY "
                        }
                    );
                    assert_eq!(found.replace, before.len()..text.len());
                }
            }
            let text = "@file.size >= 02GiB";
            let found = context(text, text.len()).unwrap();
            assert_eq!(
                found.target,
                CompletionTarget::Value {
                    field: "@file.size".into()
                }
            );
            assert_eq!(found.prefix, "02GiB");
        }
    };
}
pub(super) use completion_tests;

macro_rules! json_string_tests {
    () => {
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
    };
}
pub(super) use json_string_tests;

macro_rules! options_tests {
    () => {
        use crate::index::bigram::BigramAlphabet;

        use super::{ReturnMode, SortExpression, SortField, SortReducer};

        #[test]
        fn runtime_sort_option_types_construct_a_computed_field() {
            let mode: crate::query::options::ReturnMode = ReturnMode::Logical;
            let alphabet: crate::index::bigram::BigramAlphabet = BigramAlphabet::Unicode;
            let expression: crate::query::options::SortExpression = SortExpression {
                path: "title".into(),
                binding: None,
                reducer: Some(SortReducer::First),
            };
            let field: crate::query::options::SortField = SortField::computed(expression, false);
            assert_eq!(mode, ReturnMode::Logical);
            assert_eq!(alphabet, BigramAlphabet::Unicode);
            assert_eq!(field.field, "title");
        }
    };
}
pub(super) use options_tests;

macro_rules! path_tests {
    () => {
        use super::*;

        #[test]
        fn explicit_root_paths_keep_zero_or_more_steps() {
            let root = QueryPath::parse("@root").unwrap();
            assert_eq!(root.field.root, PathRoot::Data);
            assert!(root.field.segments.is_empty());
            assert!(root.steps.is_empty());

            let position = QueryPath::parse("@root[0]").unwrap();
            assert_eq!(position.steps, [PathStep::ArrayIndex(0)]);
            assert!(position.has_position);

            let nested = QueryPath::parse("@root[*].title").unwrap();
            assert_eq!(
                nested.steps,
                [PathStep::EachElement, PathStep::Field("title".into())]
            );
            assert_eq!(nested.field.segments, ["title"]);
            assert_eq!(
                QueryPath::parse("@root.title").unwrap().field,
                QueryPath::parse("title").unwrap().field
            );
            assert_eq!(
                QueryPath::parse("@root[\"\"]").unwrap().field.segments,
                [""]
            );
        }

        #[test]
        fn explicit_array_steps_are_distinct_from_quoted_object_keys() {
            let path = QueryPath::parse("items[0].name").unwrap();
            assert_eq!(path.field.segments, ["items", "name"]);
            assert_eq!(
                path.steps,
                [
                    PathStep::Field("items".into()),
                    PathStep::ArrayIndex(0),
                    PathStep::Field("name".into()),
                ]
            );
            assert!(path.has_position);
            let key = QueryPath::parse("items['0'].name").unwrap();
            assert_eq!(key.field.segments, ["items", "0", "name"]);
            assert_eq!(key.steps[1], PathStep::Field("0".into()));
            assert!(!key.has_position);
            let nested = QueryPath::parse("matrix[1][2]").unwrap();
            assert_eq!(
                nested.steps,
                [
                    PathStep::Field("matrix".into()),
                    PathStep::ArrayIndex(1),
                    PathStep::ArrayIndex(2),
                ]
            );
            let each = QueryPath::parse("items[*].price").unwrap();
            assert_eq!(each.steps[1], PathStep::EachElement);
            assert!(each.has_explicit_array_step);
            assert!(!each.has_position);
            assert_eq!(
                QueryPath::parse("['literal.key'][0]")
                    .unwrap()
                    .field
                    .segments,
                ["literal.key"]
            );
        }

        #[test]
        fn malformed_or_overflowing_selectors_fail_cleanly() {
            for path in [
                "items[-1]",
                "items[1.5]",
                "items[4294967296]",
                "items[",
                "items[]",
                "items[1]tail",
                "items.",
                ".items",
                "[0]",
                "items[abc]",
            ] {
                assert!(QueryPath::parse(path).is_err(), "{path}");
            }
        }

        #[test]
        fn backlink_root_is_reserved_and_keeps_explicit_positions() {
            let path = QueryPath::parse("@backlinks.mentions[0].title").unwrap();
            assert_eq!(path.field.root, PathRoot::Backlinks);
            assert_eq!(path.field.segments, ["mentions", "title"]);
            assert_eq!(path.steps[0], PathStep::Field("mentions".into()));
            assert_eq!(path.steps[1], PathStep::ArrayIndex(0));
            assert!(path.has_position);
            assert_eq!(path.display, "@backlinks.mentions[0].title");
            let quoted = QueryPath::parse("['@backlinks'].mentions.title").unwrap();
            assert_eq!(quoted.field.root, PathRoot::Data);
            assert!(QueryPath::parse("@backlinks").is_err());
            assert!(QueryPath::parse("@backlinks.").is_err());
        }

        #[test]
        fn binding_split_skips_quoted_colons_and_array_selectors() {
            for (input, path, binding) in [
                ("x:lower", "x", Some("lower")),
                ("[\"a:b\"]", "[\"a:b\"]", None),
                ("['a:b']", "['a:b']", None),
                (
                    "items[0][\"a:b\"]:lower",
                    "items[0][\"a:b\"]",
                    Some("lower"),
                ),
                ("[\"a\\\"b:c\"]:lower", "[\"a\\\"b:c\"]", Some("lower")),
                ("[\"a]b:c\"]", "[\"a]b:c\"]", None),
                ("items[*].name", "items[*].name", None),
            ] {
                assert_eq!(split_path_binding(input), (path, binding), "{input}");
            }
        }

        #[test]
        fn query_steps_and_schema_addresses_round_trip_through_their_own_rendering() {
            for input in [
                "@root",
                "@root[0]",
                "@root[*].title",
                "@file.status",
                "@backlinks.mentions[0].title",
                "['@file'].size",
                "['']",
                "items['0'].name",
                "items[0][\"a:b\"]",
                "items[*]['a b']",
                "['a>=b']",
                "['a\\'b']",
                "[\"a]b\"]",
            ] {
                let parsed = QueryPath::parse(input).unwrap();
                let again = QueryPath::parse(&parsed.display).unwrap();
                assert_eq!(again.field, parsed.field, "{input}");
                assert_eq!(again.steps, parsed.steps, "{input}");
                assert_eq!(
                    FieldPath::parse(&parsed.field.display()).unwrap(),
                    parsed.field,
                    "{input}"
                );
            }
        }
    };
}
pub(super) use path_tests;

macro_rules! regex_tests {
    () => {
        use super::*;
        use crate::index::bigram::BigramAlphabet;
        use crate::index::bigram::BigramIndex;
        use crate::model::{FieldPath, LogicalType, RecordSlot};
        use crate::query::binding::fts::bind_fts;
        use crate::query::options::ReturnMode;
        use crate::query::{FtsField, FtsMode, FtsQuery, Query, QueryOptions};
        use crate::query_execution::restrictions::fts_candidates;
        use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
        use crate::schema::{
            FieldDefinition, Schema, operation::BindingOperation, operation::CustomBinding,
            operation::RegisteredBinding, operation::Transform,
        };
        use crate::source::TextSegment;

        #[test]
        fn compilation_limits_and_flags() {
            let plain = CompiledRegex::compile(
                "^foo.bar$",
                RegexOptions::default(),
                CaseSensitivity::Sensitive,
            )
            .unwrap();
            assert!(plain.is_match("foo-bar"));
            assert!(!plain.is_match("foo\nbar"));
            assert!(!plain.is_match("FOO-BAR"));

            let insensitive = CompiledRegex::compile(
                "^foo.bar$",
                RegexOptions {
                    multi_line: true,
                    dot_matches_new_line: true,
                },
                CaseSensitivity::Insensitive,
            )
            .unwrap();
            assert!(insensitive.is_match("FOO\nBAR"));
            assert!(insensitive.is_match("prefix\nFOO-BAR\nsuffix"));

            assert!(
                CompiledRegex::compile("(", RegexOptions::default(), CaseSensitivity::Sensitive)
                    .unwrap_err()
                    .contains("invalid regex")
            );
            assert!(
                CompiledRegex::compile("a\\1", RegexOptions::default(), CaseSensitivity::Sensitive)
                    .unwrap_err()
                    .contains("invalid regex")
            );
            assert!(
                CompiledRegex::compile(
                    &"a".repeat(MAX_PATTERN_BYTES + 1),
                    RegexOptions::default(),
                    CaseSensitivity::Sensitive
                )
                .unwrap_err()
                .contains("byte limit")
            );
        }

        #[test]
        fn regex_fts_is_segment_scoped_mode_aware_and_cache_safe() {
            let root = std::env::temp_dir().join(format!("objekte-regex-fts-{}", std::process::id()));
            let data = root.join("data");
            std::fs::create_dir_all(&data).unwrap();
            let lines = [
                fixture_value!({ "title" => ("foo"), "body" => ("bar") }).fixture_text(),
                fixture_value!({ "title" => ("foo-bar"), "body" => ("x") }).fixture_text(),
                fixture_value!({ "title" => ("FOO\nBAR"), "body" => ("x") }).fixture_text(),
                fixture_value!({ "title" => ("Straße"), "body" => ("x") }).fixture_text(),
                fixture_value!({ "title" => ("MÜLLER"), "body" => ("x") }).fixture_text(),
                "(\"title\"=needle".into(),
            ];
            std::fs::write(data.join("records.rows"), lines.join("\n")).unwrap();
            let mut schema = Schema::default();
            for name in ["title", "body"] {
                schema
                    .register_field(FieldDefinition {
                        path: vec![name.into()],
                        kind: LogicalType::String,
                        required: true,
                        nullable: false,
                        indexed: false,
                        multi: false,
                        element_nullable: Vec::new(),
                    })
                    .unwrap();
            }
            schema
                .register_custom(
                    vec!["title".into()],
                    CustomBinding {
                        name: "folded".into(),
                        version: 1,
                        transform: Transform::CaseFold,
                    },
                )
                .unwrap();
            let definition = CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: crate::source::test_support::line_format(),
                identity: IdentityPolicy::Generated,
                discovery: Default::default(),
                schema,
                sort_fields: Vec::new(),
                sort_locale: "en".into(),
                sort_options: Default::default(),
                sort_overrides: Default::default(),
            };
            for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
                let repo = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        bigram_alphabet: alphabet,
                        verified_cache_bytes: 1024 * 1024,
                        ..Default::default()
                    },
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                let query = |pattern: &str, selected: Option<&str>, case_sensitivity, options| Query {
                    collections: vec!["data".into()],
                    fts: Some(FtsQuery {
                        pattern_parameter: None,
                        case_sensitivity,
                        mode: FtsMode::Regex(options),
                        literal: pattern.into(),
                        fields: selected
                            .into_iter()
                            .map(|name| FtsField {
                                query_path: None,
                                path: FieldPath::parse(name).unwrap(),
                                binding: None,
                            })
                            .collect(),
                    }),
                    filter: None,
                    options: QueryOptions {
                        sort: Vec::new(),
                        limit: 20,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Logical,
                        evidence: true,
                    },
                };
                for (request, expected) in [
                    (
                        query(
                            "^foo.bar$",
                            Some("title"),
                            CaseSensitivity::Sensitive,
                            RegexOptions::default(),
                        ),
                        1,
                    ),
                    (
                        query(
                            "^foo.bar$",
                            Some("title"),
                            CaseSensitivity::Insensitive,
                            RegexOptions {
                                multi_line: true,
                                dot_matches_new_line: true,
                            },
                        ),
                        2,
                    ),
                    (
                        query(
                            "foo.*bar",
                            None,
                            CaseSensitivity::Sensitive,
                            RegexOptions::default(),
                        ),
                        1,
                    ),
                    (
                        query(
                            "^bar$",
                            Some("body"),
                            CaseSensitivity::Sensitive,
                            RegexOptions::default(),
                        ),
                        1,
                    ),
                    (
                        query(
                            "\\(\\\"title\\\"=needle",
                            None,
                            CaseSensitivity::Sensitive,
                            RegexOptions::default(),
                        ),
                        1,
                    ),
                ] {
                    let optimized = repo.search(&request).unwrap();
                    let reference = repo.search_reference(&request).unwrap();
                    assert_eq!(optimized.count, Some(expected), "{alphabet:?} {request:?}");
                    assert_eq!(optimized.hits, reference.hits, "{alphabet:?} {request:?}");
                    assert_eq!(repo.search(&request).unwrap().hits, optimized.hits);
                }
                let selective = query(
                    "foo.*bar",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                let plan = repo.explain(&selective).unwrap();
                // Folded grams admit a mixed-case distractor; exact regex rejects it.
                assert_eq!(plan[0].fts_estimated_candidates, Some(3));
                assert_eq!(
                    plan[0].fts_mode,
                    Some(FtsMode::Regex(RegexOptions::default()))
                );
                assert!(
                    plan[0]
                        .fts_prefilter_reason
                        .as_ref()
                        .unwrap()
                        .contains("necessary folded grams")
                );
                let bound = bind_fts(selective.fts.as_ref().unwrap(), &definition.schema).unwrap();
                assert_eq!(
                    repo.segments[0]
                        .iter()
                        .map(|segment| {
                            crate::query_execution::restrictions::segment_fts_candidates(
                                segment.id,
                                &segment.index,
                                &repo.collections()[0],
                                &bound,
                            )
                            .unwrap()
                            .unwrap_or_else(|| segment.active.bitmap().unwrap())
                            .len()
                        })
                        .sum::<u64>(),
                    3
                );
                // The record with separate "foo" and "bar" segments is a candidate,
                // but cannot satisfy the exact regex within either segment.
                assert_eq!(repo.search(&selective).unwrap().count, Some(1));
                let unrestricted = query(
                    "foo|.*",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                let plan = repo.explain(&unrestricted).unwrap();
                assert!(plan[0].fts_estimated_candidates.is_some());
                assert!(
                    plan[0]
                        .fts_prefilter_reason
                        .as_ref()
                        .unwrap()
                        .contains("no bounded necessary literal")
                );
                let sensitive = query(
                    "^foo",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                let insensitive = query(
                    "^foo",
                    Some("title"),
                    CaseSensitivity::Insensitive,
                    RegexOptions::default(),
                );
                assert_eq!(repo.search(&sensitive).unwrap().count, Some(2));
                let mut folded = query(
                    "^strasse$",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                folded.fts.as_mut().unwrap().fields[0].binding = Some("folded".into());
                assert_eq!(repo.search(&folded).unwrap().count, Some(1));
                assert_eq!(
                    repo.search(&folded).unwrap().hits,
                    repo.search_reference(&folded).unwrap().hits
                );
                let bound = bind_fts(folded.fts.as_ref().unwrap(), &definition.schema).unwrap();
                assert!(fts_candidates(&repo.collections()[0], &bound).is_some());
                let fallback = query(
                    "\\(\\\"title\\\"=needle",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                let bound = bind_fts(fallback.fts.as_ref().unwrap(), &definition.schema).unwrap();
                assert!(repo.segments[0].iter().any(|segment| {
                    let candidates = crate::query_execution::restrictions::segment_fts_candidates(
                        segment.id,
                        &segment.index,
                        &repo.collections()[0],
                        &bound,
                    )
                    .unwrap()
                    .unwrap_or_else(|| segment.active.bitmap().unwrap().clone());
                    segment
                        .native_payload()
                        .records
                        .iter()
                        .enumerate()
                        .any(|(slot, record)| {
                            record.class == crate::inventory::RecordClass::RawFallback
                                && candidates.contains(slot as u32)
                        })
                }));
                assert_eq!(repo.search(&insensitive).unwrap().count, Some(3));
                let inline_case = query(
                    "(?i:^foo)",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                assert_eq!(repo.search(&inline_case).unwrap().count, Some(3));
                let unicode_case = query(
                    "(?i:^müller$)",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                assert_eq!(repo.search(&unicode_case).unwrap().count, Some(1));
                assert_eq!(
                    repo.search(&unicode_case).unwrap().hits,
                    repo.search_reference(&unicode_case).unwrap().hits
                );
                assert_eq!(repo.search(&sensitive).unwrap().count, Some(2));
                let invalid = query(
                    "(",
                    None,
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                assert!(repo.search(&invalid).unwrap_err().contains("invalid regex"));
                drop(repo);
                let restored = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        bigram_alphabet: alphabet,
                        ..Default::default()
                    },
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                let request = query(
                    "^foo.bar$",
                    Some("title"),
                    CaseSensitivity::Sensitive,
                    RegexOptions::default(),
                );
                assert_eq!(restored.search(&request).unwrap().count, Some(1));
                assert_eq!(
                    restored.search(&request).unwrap().hits,
                    restored.search_reference(&request).unwrap().hits
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn generated_regex_matches_remain_inside_conservative_gram_candidates() {
            let fixed: Vec<Vec<&str>> = vec![
                vec![""],
                vec!["foo"],
                vec!["bar"],
                vec!["foobar"],
                vec!["fooXbar"],
                vec!["FOObar"],
                vec!["Straße"],
                vec!["STRASSE"],
                vec!["ß"],
                vec!["ss"],
                vec!["aßb"],
                vec!["abc"],
                vec!["aübc"],
                vec!["a😀bc"],
                vec!["Σσς"],
                vec!["Iİıi"],
                vec!["a\nb"],
                vec!["foo", "bar"],
                vec!["ab", "cd"],
            ];
            let mut sources: Vec<Vec<String>> = fixed
                .into_iter()
                .map(|parts| parts.into_iter().map(str::to_owned).collect())
                .collect();
            const SEED: u64 = 0x5EED_2026_1106;
            let mut state = SEED;
            let chars = ['a', 'b', 'ß', 'ü', 'Σ', '😀'];
            for _ in 0..128 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let length = ((state >> 32) % 9) as usize;
                let mut text = String::new();
                for _ in 0..length {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    text.push(chars[((state >> 32) as usize) % chars.len()]);
                }
                sources.push(vec![text]);
            }
            let patterns = [
                "foo.*bar",
                "foo|bar",
                "(?:foo)?bar",
                "ß",
                "ss",
                "aßb",
                "^abc$",
                ".*",
                "a?",
                r"\bfoo\b",
                "I|İ|ı",
                "Σ|σ|ς",
                "a.b",
                "a\\nb",
                "(?i:foo)",
                "foo|.*",
                "^foo|bar$",
            ];
            for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
                let mut index = BigramIndex::new(alphabet);
                for (slot, parts) in sources.iter().enumerate() {
                    let segments = parts
                        .iter()
                        .map(|part| TextSegment {
                            path: vec!["body".into()],
                            text: part.clone(),
                        })
                        .collect::<Vec<_>>();
                    index
                        .add_record(RecordSlot(slot as u32), &segments)
                        .unwrap();
                }
                for pattern in patterns {
                    for case_sensitivity in [CaseSensitivity::Sensitive, CaseSensitivity::Insensitive] {
                        let compiled =
                            CompiledRegex::compile(pattern, RegexOptions::default(), case_sensitivity)
                                .unwrap();
                        let Some(candidates) = compiled.condition().and_then(|condition| {
                            condition
                                .candidates(&mut |request| {
                                    Ok::<_, std::convert::Infallible>(index.candidates_prepared(request))
                                })
                                .unwrap()
                        }) else {
                            continue;
                        };
                        let independent = regex::RegexBuilder::new(pattern)
                            .case_insensitive(case_sensitivity == CaseSensitivity::Insensitive)
                            .build()
                            .unwrap();
                        for (slot, parts) in sources.iter().enumerate() {
                            if parts.iter().any(|part| independent.is_match(part)) {
                                assert!(
                                    candidates.contains(slot as u32),
                                    "seed={SEED:x} {alphabet:?} {case_sensitivity:?} {pattern:?} {parts:?}"
                                );
                            }
                        }
                    }
                }
            }
        }

        #[derive(Debug)]
        struct RegexMapping {
            compatible: bool,
        }

        impl BindingOperation for RegexMapping {
            fn accepts(
                &self,
                _kind: &LogicalType,
                _operator: crate::query::Operator,
                _multi: bool,
            ) -> bool {
                true
            }

            fn prepare_literal(
                &self,
                _literal: &crate::query::Literal,
                _kind: &LogicalType,
            ) -> Result<crate::model::Value, String> {
                Err("not used by regex".into())
            }

            fn map_value(&self, _value: &crate::model::Value) -> Option<crate::model::Value> {
                None
            }

            fn map_text(&self, text: &str) -> String {
                text.replace('-', "")
            }

            fn regex_text_compatible(&self) -> bool {
                self.compatible
            }
        }

        #[test]
        fn custom_regex_binding_requires_explicit_representation_contract() {
            let fts = FtsQuery {
                pattern_parameter: None,
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Regex(RegexOptions::default()),
                literal: "foobar".into(),
                fields: Vec::new(),
            };
            for compatible in [false, true] {
                let mut schema = Schema::default();
                schema
                    .set_raw_binding(RegisteredBinding {
                        name: "dashless".into(),
                        version: 1,
                        operation: Arc::new(RegexMapping { compatible }),
                    })
                    .unwrap();
                if compatible {
                    let bound = bind_fts(&fts, &schema).unwrap();
                    assert!(bound.raw_matcher.unwrap().matches("foo-bar").unwrap());
                    assert!(bound.fallback_regex.as_ref().unwrap().is_match("foobar"));
                    assert!(!bound.fallback_regex.as_ref().unwrap().is_match("foo-bar"));
                } else {
                    assert!(
                        bind_fts(&fts, &schema)
                            .unwrap_err()
                            .contains("does not support regex mode")
                    );
                }
            }
        }

        #[test]
        fn custom_raw_regex_uses_mapped_index_and_original_fallback_after_restore() {
            let root = std::env::temp_dir().join(format!("objekte-regex-raw-{}", std::process::id()));
            std::fs::create_dir_all(root.join("data")).unwrap();
            std::fs::write(
                root.join("data/records.rows"),
                [
                    fixture_value!({ "title" => ("foo-bar") }).fixture_text(),
                    fixture_value!({ "title" => ("fooXbar") }).fixture_text(),
                    "(\"title\"=\"foobar".into(),
                ]
                .join("\n"),
            )
            .unwrap();
            let mut schema = Schema::default();
            schema
                .set_raw_binding(RegisteredBinding {
                    name: "dashless".into(),
                    version: 1,
                    operation: Arc::new(RegexMapping { compatible: true }),
                })
                .unwrap();
            let definition = CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: crate::source::test_support::line_format(),
                identity: IdentityPolicy::Generated,
                discovery: Default::default(),
                schema,
                sort_fields: Vec::new(),
                sort_locale: "en".into(),
                sort_options: Default::default(),
                sort_overrides: Default::default(),
            };
            let query = |pattern: &str| Query {
                collections: vec!["data".into()],
                fts: Some(FtsQuery {
                    pattern_parameter: None,
                    case_sensitivity: CaseSensitivity::Sensitive,
                    mode: FtsMode::Regex(RegexOptions::default()),
                    literal: pattern.into(),
                    fields: Vec::new(),
                }),
                filter: None,
                options: QueryOptions {
                    sort: Vec::new(),
                    limit: 10,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Logical,
                    evidence: true,
                },
            };
            for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
                let options = RepositoryOptions {
                    bigram_alphabet: alphabet,
                    verified_cache_bytes: 1024 * 1024,
                    ..Default::default()
                };
                let built = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    options,
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                assert_eq!(built.build_failed_parses(), 1);
                let built_query = query("foobar");
                assert_eq!(built.search(&built_query).unwrap().count, Some(2));
                assert_eq!(
                    built.search(&built_query).unwrap().hits,
                    built.search_reference(&built_query).unwrap().hits
                );
                drop(built);
                let restored = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    options,
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                let collection = &restored.collections()[0];
                for (pattern, expected) in [("^foobar$", 1), ("foobar", 2), ("foo-bar", 0)] {
                    let request = query(pattern);
                    let bound = bind_fts(request.fts.as_ref().unwrap(), &definition.schema).unwrap();
                    let candidates = restored.segments[0]
                        .iter()
                        .map(|segment| {
                            let candidates = crate::query_execution::restrictions::segment_fts_candidates(
                                segment.id,
                                &segment.index,
                                collection,
                                &bound,
                            )
                            .unwrap()
                            .unwrap_or_else(|| segment.active.bitmap().unwrap());
                            (segment, candidates)
                        })
                        .collect::<Vec<_>>();
                    if pattern == "foobar" {
                        assert_eq!(
                            candidates
                                .iter()
                                .map(|(_, candidates)| candidates.len())
                                .sum::<u64>(),
                            2,
                            "{alphabet:?}"
                        );
                        assert!(candidates.iter().any(|(segment, candidates)| {
                            segment.native_payload().records.iter().any(|record| {
                                record.class == crate::inventory::RecordClass::RawFallback
                                    && candidates.contains(record.slot.0)
                            })
                        }));
                    }
                    let optimized = restored.search(&request).unwrap();
                    let reference = restored.search_reference(&request).unwrap();
                    assert_eq!(optimized.count, Some(expected), "{alphabet:?} {pattern}");
                    assert_eq!(optimized.hits, reference.hits, "{alphabet:?} {pattern}");
                    if pattern == "foobar" {
                        assert!(optimized.hits.iter().any(|hit| {
                            hit.record_class == crate::inventory::RecordClass::RawFallback
                                && hit.matched_fields == ["@record.raw_text"]
                        }));
                    }
                }
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        use crate::source::test_support::{FixtureText, fixture_value};
    };
}
pub(super) use regex_tests;

macro_rules! size_literal_tests {
    () => {
        use super::parse_size_literal;

        #[test]
        fn si_iec_and_fractional_sizes_are_exact() {
            for (input, expected) in [
                ("0B", 0),
                ("1536B", 1536),
                ("2.5KB", 2500),
                ("1.25MB", 1_250_000),
                ("1GB", 1_000_000_000),
                ("1TB", 1_000_000_000_000),
                ("1KiB", 1024),
                ("1.5MiB", 1_572_864),
                ("2GiB", 2_147_483_648),
                ("1TiB", 1_099_511_627_776),
                ("0.0009765625KiB", 1),
                ("0.00000000001818989403545856475830078125TiB", 20),
                ("0.000000000000000000000000000000000000000000000000B", 0),
            ] {
                assert_eq!(
                    parse_size_literal(input).unwrap(),
                    Some(expected),
                    "{input}"
                );
            }
            assert_eq!(
                parse_size_literal("170141183460469231731687303715884105727B").unwrap(),
                Some(i128::MAX)
            );
        }

        #[test]
        fn invalid_sizes_do_not_round_or_wrap() {
            for input in [
                "0.1B",
                "1.2345KB",
                "-1MiB",
                "1mib",
                "1Mb",
                "1PB",
                "1.2.3MiB",
                "1.MiB",
                "170141183460469231731687303715884105728B",
                "1e3B",
            ] {
                assert!(parse_size_literal(input).is_err(), "{input}");
            }
            for input in ["123", "2.5", "1e3", "name", "@file.size"] {
                assert_eq!(parse_size_literal(input).unwrap(), None, "{input}");
            }
        }
    };
}
pub(super) use size_literal_tests;

macro_rules! sort_binding_tests {
    () => {
        use super::*;
        use crate::index::sorting::Placement;
        use crate::model::{ArrayValue, LogicalType, Number};
        use crate::query::QueryOptions;
        use crate::query::options::ReturnMode;
        use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
        use crate::schema::relations::{RelationCardinality, RelationDefinition};
        use crate::schema::{FieldDefinition, operation::RegisteredBinding};

        fn stored_string_sort_definition(
            name: &str,
            defaults: SortOptions,
            effective: Option<SortConfig>,
        ) -> CollectionDefinition {
            let mut definition = crate::source::test_support::document_folder(name, name);
            definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            definition.sort_locale = "en".into();
            definition.sort_options = defaults;
            for (path, kind, nullable) in [
                ("id", LogicalType::Id, false),
                ("title", LogicalType::String, true),
            ] {
                definition
                    .schema
                    .register_field(FieldDefinition {
                        path: vec![path.into()],
                        kind,
                        required: !nullable,
                        nullable,
                        indexed: false,
                        multi: false,
                        element_nullable: vec![],
                    })
                    .unwrap();
            }
            let path = QueryPath::parse("title").unwrap().field;
            definition.sort_fields.push(path.clone());
            if let Some(effective) = effective {
                definition.sort_overrides.insert(path, effective);
            }
            definition
        }

        fn stored_title_query(collections: &[&str]) -> Query {
            Query {
                collections: collections.iter().map(|name| (*name).into()).collect(),
                filter: None,
                fts: None,
                options: QueryOptions {
                    sort: vec![SortField::simple("title", false)],
                    limit: 20,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            }
        }

        fn hit_ids(result: &crate::query_results::SearchResult) -> Vec<&str> {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect()
        }

        #[test]
        fn reference_stored_sort_uses_numeric_and_null_override() {
            let root = std::env::temp_dir().join(format!(
                "objekte-sort-binding-collation-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir_all(root.join("data")).unwrap();
            for (name, document) in [
                ("ten", r#"("id"="ten","title"="item10")"#),
                ("two", r#"("id"="two","title"="item2")"#),
                ("null", r#"("id"="null","title"=null)"#),
            ] {
                std::fs::write(root.join(format!("data/{name}.row")), document).unwrap();
            }
            let defaults = SortOptions {
                numeric: false,
                nulls: Placement::Last,
                missing: Placement::Last,
                ..Default::default()
            };
            let effective = SortConfig {
                locale: "en".into(),
                options: SortOptions {
                    numeric: true,
                    nulls: Placement::First,
                    missing: Placement::First,
                    ..Default::default()
                },
            };
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![stored_string_sort_definition(
                    "data",
                    defaults,
                    Some(effective),
                )],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let query = stored_title_query(&["data"]);

            let optimized = repo.search(&query).unwrap();
            let reference = repo.search_reference(&query).unwrap();
            assert_eq!(hit_ids(&optimized), ["null", "two", "ten"]);
            assert_eq!(hit_ids(&reference), hit_ids(&optimized));
            assert_eq!(optimized.count, Some(3));
            assert_eq!(reference.count, optimized.count);

            drop(repo);
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn cross_collection_sort_uses_effective_override_compatibility() {
            let root = std::env::temp_dir().join(format!(
                "objekte-sort-binding-cross-collation-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            for collection in ["a", "b"] {
                std::fs::create_dir_all(root.join(collection)).unwrap();
                std::fs::write(
                    root.join(collection).join("ten.row"),
                    format!(r#"("id"="{collection}10","title"="item10")"#),
                )
                .unwrap();
                std::fs::write(
                    root.join(collection).join("two.row"),
                    format!(r#"("id"="{collection}2","title"="item2")"#),
                )
                .unwrap();
            }
            let defaults_a = SortOptions {
                numeric: false,
                nulls: Placement::Last,
                missing: Placement::Last,
                ..Default::default()
            };
            let defaults_b = SortOptions {
                numeric: false,
                nulls: Placement::First,
                missing: Placement::First,
                ..Default::default()
            };
            let numeric = SortConfig {
                locale: "en".into(),
                options: SortOptions {
                    numeric: true,
                    nulls: Placement::First,
                    missing: Placement::First,
                    ..Default::default()
                },
            };
            let equal = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![
                    stored_string_sort_definition("a", defaults_a, Some(numeric.clone())),
                    stored_string_sort_definition("b", defaults_b, Some(numeric.clone())),
                ],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let query = stored_title_query(&["a", "b"]);
            let optimized = equal.search(&query).unwrap();
            let reference = equal.search_reference(&query).unwrap();
            assert_eq!(hit_ids(&optimized), ["a2", "b2", "a10", "b10"]);
            assert_eq!(hit_ids(&reference), hit_ids(&optimized));
            drop(equal);

            let nonnumeric = SortConfig {
                locale: "en".into(),
                options: defaults_a,
            };
            let differing = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![
                    stored_string_sort_definition("a", defaults_a, Some(numeric)),
                    stored_string_sort_definition("b", defaults_a, Some(nonnumeric)),
                ],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            assert!(
                differing
                    .search(&query)
                    .unwrap_err()
                    .contains("incompatible cross-collection sort")
            );
            assert!(
                differing
                    .search_reference(&query)
                    .unwrap_err()
                    .contains("incompatible cross-collection sort")
            );

            drop(differing);
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn optimized_binding_defers_reference_sort_preparation() {
            let root = std::env::temp_dir().join(format!(
                "objekte-sort-binding-reference-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir_all(root.join("data")).unwrap();
            std::fs::write(root.join("data/a.row"), r#"("title"="Alpha")"#).unwrap();
            let mut definition = crate::source::test_support::document_folder("data", "data");
            definition
                .schema
                .register_field(FieldDefinition {
                    path: vec!["title".into()],
                    kind: LogicalType::String,
                    required: true,
                    nullable: false,
                    indexed: false,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
            definition
                .sort_fields
                .push(QueryPath::parse("title").unwrap().field);
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![definition],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let query = Query {
                collections: vec!["data".into()],
                filter: None,
                fts: None,
                options: QueryOptions {
                    sort: vec![SortField::simple("title", false)],
                    limit: 10,
                    offset: 0,
                    count: false,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            };

            let before = BoundSort::reference_stored_sort_preparations();
            let optimized = BoundSort::bind(&repo, &query).unwrap();
            assert_eq!(
                BoundSort::reference_stored_sort_preparations(),
                before,
                "optimized stored ordering must not prepare a reference collator"
            );
            assert!(matches!(
                optimized.terms[0].as_ref().unwrap()[0].source,
                BoundSortSource::Stored(_)
            ));

            let reference = BoundSort::bind_reference(&repo, &query).unwrap();
            assert_eq!(
                BoundSort::reference_stored_sort_preparations(),
                before + 1,
                "reference binding prepares one source evaluator"
            );
            assert!(matches!(
                reference.terms[0].as_ref().unwrap()[0].source,
                BoundSortSource::Computed(_)
            ));

            drop(repo);
            std::fs::remove_dir_all(root).unwrap();
        }

        #[derive(Debug)]
        struct NullAsZero;

        impl BindingOperation for NullAsZero {
            fn accepts(&self, _: &crate::model::LogicalType, _: crate::query::Operator, _: bool) -> bool {
                true
            }

            fn prepare_literal(
                &self,
                _: &crate::query::Literal,
                _: &crate::model::LogicalType,
            ) -> Result<Value, String> {
                Ok(Value::Null)
            }

            fn map_value(&self, value: &Value) -> Option<Value> {
                Some(if matches!(value, Value::Null) {
                    Value::Number(Number::integer(0))
                } else {
                    value.clone()
                })
            }

            fn map_text(&self, text: &str) -> String {
                text.into()
            }

            fn sort_compatible(&self) -> bool {
                true
            }
        }

        #[test]
        fn min_max_transform_null_before_classifying_it() {
            let null = Value::Null;
            let two = Value::Number(Number::integer(2));
            let values = [&null, &two];
            for (reducer, expected) in [
                (SortReducer::Min, Number::integer(0)),
                (SortReducer::Max, Number::integer(2)),
                (SortReducer::First, Number::integer(0)),
                (SortReducer::Last, Number::integer(2)),
            ] {
                let mut sort = sorter("values[*]", Some(reducer));
                sort.transform = SortTransform::Operation(Arc::new(NullAsZero));
                assert_eq!(
                    sort.reduce(&values, true).unwrap(),
                    SortKey::Number(expected)
                );
            }
        }

        #[test]
        fn cross_collection_sort_rejects_different_binding_versions() {
            let root = std::env::temp_dir().join(format!(
                "objekte-sort-binding-version-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            let mut definitions = Vec::new();
            for (name, version) in [("a", 1), ("b", 2)] {
                std::fs::create_dir_all(root.join(name)).unwrap();
                std::fs::write(root.join(name).join("record.row"), r#"("title"="Alpha")"#).unwrap();
                let mut definition = crate::source::test_support::document_folder(name, name);
                definition
                    .schema
                    .register_field(FieldDefinition {
                        path: vec!["title".into()],
                        kind: LogicalType::String,
                        required: true,
                        nullable: false,
                        indexed: false,
                        multi: false,
                        element_nullable: vec![],
                    })
                    .unwrap();
                definition
                    .schema
                    .register_operation(
                        vec!["title".into()],
                        RegisteredBinding {
                            name: "normalized".into(),
                            version,
                            operation: Arc::new(NullAsZero),
                        },
                    )
                    .unwrap();
                definitions.push(definition);
            }
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                definitions,
                RepositoryOptions {
                    ..Default::default()
                },
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let query = Query {
                collections: Vec::new(),
                filter: None,
                fts: None,
                options: QueryOptions {
                    sort: vec![SortField::computed(
                        SortExpression {
                            path: "title".into(),
                            binding: Some("normalized".into()),
                            reducer: None,
                        },
                        false,
                    )],
                    limit: 10,
                    offset: 0,
                    count: false,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            };
            assert!(
                repo.search(&query)
                    .unwrap_err()
                    .contains("incompatible cross-collection sort")
            );
            drop(repo);
            std::fs::remove_dir_all(root).unwrap();
        }

        fn sorter(path: &str, reducer: Option<SortReducer>) -> ComputedSort {
            let sort_config = SortConfig {
                locale: "und".into(),
                options: SortOptions::default(),
            };
            ComputedSort {
                path: QueryPath::parse(path).unwrap(),
                related: None,
                reducer,
                transform: SortTransform::Identity,
                collator: make_collator(&sort_config).unwrap(),
                sort_config,
            }
        }

        #[test]
        fn explicit_positions_and_reductions_keep_occurrence_order() {
            let prices = Value::Array(
                ArrayValue::new(
                    vec![
                        Value::Number(Number::integer(3)),
                        Value::Number(Number::integer(1)),
                        Value::Number(Number::integer(3)),
                    ],
                    None,
                    false,
                )
                .unwrap(),
            );
            let mut object = std::collections::BTreeMap::new();
            object.insert("prices".into(), prices);
            let value = Value::Object(object);
            let mut selected = Vec::new();
            let mut expanded = false;
            assert!(select_values(
                &value,
                &QueryPath::parse("prices[1]").unwrap().steps,
                &mut selected,
                &mut expanded
            ));
            assert_eq!(
                sorter("prices[1]", None)
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Number(Number::integer(1))
            );
            selected.clear();
            assert!(select_values(
                &value,
                &QueryPath::parse("prices[*]").unwrap().steps,
                &mut selected,
                &mut expanded
            ));
            assert!(
                sorter("prices[*]", None)
                    .reduce(&selected, expanded)
                    .is_err()
            );
            assert_eq!(
                sorter("prices[*]", Some(SortReducer::Min))
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Number(Number::integer(1))
            );
            assert_eq!(
                sorter("prices[*]", Some(SortReducer::Max))
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Number(Number::integer(3))
            );
            assert_eq!(
                sorter("prices[*]", Some(SortReducer::First))
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Number(Number::integer(3))
            );
            assert_eq!(
                sorter("prices[*]", Some(SortReducer::Last))
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Number(Number::integer(3))
            );
        }

        #[test]
        fn empty_ancestor_is_missing_but_nonempty_missing_child_is_absent() {
            let mut empty = std::collections::BTreeMap::new();
            empty.insert(
                "items".into(),
                Value::Array(ArrayValue::new(vec![], None, false).unwrap()),
            );
            let path = QueryPath::parse("items.title");
            let mut selected = Vec::new();
            let mut expanded = false;
            let value = Value::Object(empty);
            assert!(select_values(
                &value,
                &path.unwrap().steps,
                &mut selected,
                &mut expanded
            ));
            assert!(selected.is_empty());
            assert_eq!(
                sorter("items.title", Some(SortReducer::Min))
                    .reduce(&selected, expanded)
                    .unwrap(),
                SortKey::Missing
            );
        }

        #[test]
        fn untyped_sort_names_remain_literal_and_typed_expressions_select_bindings() {
            let untyped = expression(&SortField::simple("x:lower", false)).unwrap();
            assert_eq!(untyped.path, "x:lower");
            assert_eq!(untyped.binding, None);

            let typed = expression(&SortField::computed(
                SortExpression {
                    path: "x".into(),
                    binding: Some("lower".into()),
                    reducer: None,
                },
                false,
            ))
            .unwrap();
            assert_eq!(typed.path, "x");
            assert_eq!(typed.binding.as_deref(), Some("lower"));

            let legacy_positional = expression(&SortField::simple("items[0]:lower", false)).unwrap();
            assert_eq!(legacy_positional.path, "items[0]");
            assert_eq!(legacy_positional.binding.as_deref(), Some("lower"));

            let quoted = expression(&SortField::simple("[\"a:b\"]", false)).unwrap();
            assert_eq!(quoted.path, "[\"a:b\"]");
            assert_eq!(quoted.binding, None);
        }

        fn q4_field(schema: &mut crate::schema::Schema, name: &str, kind: LogicalType) {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind,
                    required: false,
                    nullable: true,
                    indexed: false,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }

        #[test]
        fn related_sort_reuses_targets_across_worker_counts_and_source_layouts() {
            for split_files in [false, true] {
                for high_reuse in [false, true] {
                    let root = std::env::temp_dir().join(format!(
                        "objekte-sort-target-workers-{}-{split_files}-{high_reuse}",
                        std::process::id()
                    ));
                    let source = root.join("sources");
                    let target = root.join("targets");
                    std::fs::create_dir_all(&source).unwrap();
                    std::fs::create_dir_all(&target).unwrap();
                    let target_count = if high_reuse { 4 } else { 47 };
                    for index in 0..target_count {
                        std::fs::write(
                            target.join(format!("{index:03}.row")),
                            format!(
                                "(\"id\"=\"T{index:03}\",\"title\"=\"Title-{:03}\")",
                                (index * 13) % target_count
                            ),
                        )
                        .unwrap();
                    }
                    let rows = (0..48)
                        .map(|index| {
                            let entry = if index == 47 {
                                "dangling".to_string()
                            } else {
                                format!("T{:03}", index % target_count)
                            };
                            format!("(\"id\"=\"S{index:03}\",\"entry\"=\"{entry}\")\n")
                        })
                        .collect::<Vec<_>>();
                    if split_files {
                        for (index, row) in rows.iter().enumerate() {
                            std::fs::write(source.join(format!("{index:03}.rows")), row).unwrap();
                        }
                    } else {
                        std::fs::write(source.join("all.rows"), rows.concat()).unwrap();
                    }
                    let mut source_definition = CollectionDefinition::folder(
                        "sources",
                        "sources",
                        crate::source::test_support::line_format(),
                    );
                    source_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
                    q4_field(&mut source_definition.schema, "id", LogicalType::Id);
                    q4_field(&mut source_definition.schema, "entry", LogicalType::Id);
                    source_definition
                        .schema
                        .register_relation(RelationDefinition {
                            path: vec!["entry".into()],
                            target_collection: "targets".into(),
                            cardinality: RelationCardinality::One,
                            reverse_alias: None,
                        })
                        .unwrap();
                    let mut target_definition =
                        crate::source::test_support::document_folder("targets", "targets");
                    target_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
                    q4_field(&mut target_definition.schema, "id", LogicalType::Id);
                    q4_field(&mut target_definition.schema, "title", LogicalType::String);
                    let query = Query {
                        collections: vec!["sources".into()],
                        filter: None,
                        fts: None,
                        options: QueryOptions {
                            sort: vec![SortField::computed(
                                SortExpression {
                                    path: "entry.title".into(),
                                    binding: None,
                                    reducer: None,
                                },
                                false,
                            )],
                            limit: 15,
                            offset: 5,
                            count: true,
                            return_mode: ReturnMode::Logical,
                            evidence: false,
                        },
                    };
                    for workers in [1, 2, 4, 8] {
                        let repo = crate::lifecycle::Index::build_repository_for_test(
                            &root,
                            vec![source_definition.clone(), target_definition.clone()],
                            RepositoryOptions {
                                query_workers: workers,
                                ..Default::default()
                            },
                            crate::source::test_support::fixture_registry(),
                        )
                        .map_err(|error| error.to_string())
                        .unwrap();
                        let actual = repo.search(&query).unwrap();
                        let expected = repo.search_reference(&query).unwrap();
                        assert_eq!(actual.count, Some(48));
                        assert_eq!(actual.hits, expected.hits);
                        assert_eq!(actual.counters.sort_target_reads, target_count as u64);
                        assert_eq!(
                            actual.counters.sort_target_reuse,
                            (47 - target_count) as u64
                        );
                    }
                    std::fs::remove_dir_all(root).unwrap();
                }
            }
        }

        #[test]
        fn related_stored_sort_uses_the_target_fields_effective_collation() {
            let root = std::env::temp_dir().join(format!(
                "objekte-related-target-collation-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("sources")).unwrap();
            std::fs::create_dir_all(root.join("targets")).unwrap();
            for (id, title) in [("T2", "item2"), ("T10", "item10")] {
                std::fs::write(
                    root.join("targets").join(format!("{id}.row")),
                    format!(r#"("id"="{id}","title"="{title}")"#),
                )
                .unwrap();
            }
            for (id, target) in [("S2", "T2"), ("S10", "T10")] {
                std::fs::write(
                    root.join("sources").join(format!("{id}.row")),
                    format!(r#"("id"="{id}","entry"="{target}")"#),
                )
                .unwrap();
            }

            let nonnumeric = SortOptions {
                numeric: false,
                ..Default::default()
            };
            let numeric = SortConfig {
                locale: "en".into(),
                options: SortOptions {
                    numeric: true,
                    ..Default::default()
                },
            };
            let target = stored_string_sort_definition("targets", nonnumeric, Some(numeric.clone()));
            let mut source = crate::source::test_support::document_folder("sources", "sources");
            source.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            for (path, kind) in [("id", LogicalType::Id), ("entry", LogicalType::Id)] {
                source
                    .schema
                    .register_field(FieldDefinition {
                        path: vec![path.into()],
                        kind,
                        required: true,
                        nullable: false,
                        indexed: false,
                        multi: false,
                        element_nullable: vec![],
                    })
                    .unwrap();
            }
            source
                .schema
                .register_relation(RelationDefinition {
                    path: vec!["entry".into()],
                    target_collection: "targets".into(),
                    cardinality: RelationCardinality::One,
                    reverse_alias: None,
                })
                .unwrap();
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![source.clone(), target],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let request = |limit| Query {
                collections: vec!["sources".into()],
                filter: None,
                fts: None,
                options: QueryOptions {
                    sort: vec![SortField::computed(
                        SortExpression {
                            path: "entry.title".into(),
                            binding: None,
                            reducer: None,
                        },
                        false,
                    )],
                    limit,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            };
            let bound = BoundSort::bind_reference(&repo, &request(1)).unwrap();
            let BoundSortSource::Computed(computed) = &bound.terms[0].as_ref().unwrap()[0].source else {
                panic!("related sort must use a computed source evaluator");
            };
            assert!(!computed.sort_config.options.numeric);
            let key_two = computed.scalar_key(&Value::String("item2".into())).unwrap();
            let key_ten = computed
                .scalar_key(&Value::String("item10".into()))
                .unwrap();
            assert!(matches!(
                (key_two, key_ten),
                (SortKey::String(two), SortKey::String(ten)) if ten < two
            ));
            let direct = stored_title_query(&["targets"]);
            assert_eq!(hit_ids(&repo.search(&direct).unwrap()), ["T2", "T10"]);
            for limit in [1, 20] {
                let expected = if limit == 1 {
                    vec!["S10"]
                } else {
                    vec!["S10", "S2"]
                };
                let optimized = repo.search(&request(limit)).unwrap();
                let reference = repo.search_reference(&request(limit)).unwrap();
                assert_eq!(optimized.count, Some(2));
                assert_eq!(reference.count, Some(2));
                assert_eq!(
                    hit_ids(&optimized),
                    expected,
                    "reference returned {:?}",
                    hit_ids(&reference)
                );
                assert_eq!(hit_ids(&reference), expected);
            }

            let optimized = repo.search(&direct).unwrap();
            let reference = repo.search_reference(&direct).unwrap();
            assert_eq!(hit_ids(&optimized), ["T2", "T10"]);
            assert_eq!(hit_ids(&reference), ["T2", "T10"]);
            assert_eq!(optimized.count, Some(2));
            assert_eq!(reference.count, Some(2));

            assert_eq!(
                repo.search(&request(20))
                    .unwrap()
                    .counters
                    .sort_target_reads,
                2,
                "an incompatible stored key must fall back to computed target evaluation"
            );
            drop(repo);

            let compatible = stored_string_sort_definition("targets", numeric.options, None);
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![source, compatible],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let optimized = repo.search(&request(20)).unwrap();
            let reference = repo.search_reference(&request(20)).unwrap();
            assert_eq!(hit_ids(&optimized), ["S2", "S10"]);
            assert_eq!(hit_ids(&reference), ["S2", "S10"]);
            assert_eq!(
                optimized.counters.sort_target_reads, 0,
                "compatible related stored keys must not read target records"
            );
            assert!(reference.counters.sort_target_reads > 0);

            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn related_first_last_keep_occurrence_and_dangling_order_in_parallel() {
            let root =
                std::env::temp_dir().join(format!("objekte-sort-target-order-{}", std::process::id()));
            std::fs::create_dir_all(root.join("sources")).unwrap();
            std::fs::create_dir_all(root.join("targets")).unwrap();
            for (id, title) in [("T1", "Alpha"), ("T2", "Zulu"), ("T3", "Beta")] {
                std::fs::write(
                    root.join("targets").join(format!("{id}.row")),
                    format!("(\"id\"=\"{id}\",\"title\"=\"{title}\")"),
                )
                .unwrap();
            }
            let links = [
                "[\"T2\",\"T1\"]",
                "[\"dangling\",\"T1\"]",
                "[\"T1\",\"dangling\"]",
                "[\"T3\"]",
                "[]",
            ];
            for index in 0..20 {
                std::fs::write(
                    root.join("sources").join(format!("{index:03}.rows")),
                    format!(
                        "(\"id\"=\"S{index:03}\",\"links\"={})\n",
                        links[index % links.len()]
                    ),
                )
                .unwrap();
            }
            let mut source_definition = CollectionDefinition::folder(
                "sources",
                "sources",
                crate::source::test_support::line_format(),
            );
            source_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            q4_field(&mut source_definition.schema, "id", LogicalType::Id);
            source_definition
                .schema
                .register_field(FieldDefinition {
                    path: vec!["links".into()],
                    kind: LogicalType::Array(Box::new(LogicalType::Id)),
                    required: false,
                    nullable: true,
                    indexed: false,
                    multi: true,
                    element_nullable: vec![false],
                })
                .unwrap();
            source_definition
                .schema
                .register_relation(RelationDefinition {
                    path: vec!["links".into()],
                    target_collection: "targets".into(),
                    cardinality: RelationCardinality::Many,
                    reverse_alias: None,
                })
                .unwrap();
            let mut target_definition = crate::source::test_support::document_folder("targets", "targets");
            target_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            q4_field(&mut target_definition.schema, "id", LogicalType::Id);
            q4_field(&mut target_definition.schema, "title", LogicalType::String);
            for reducer in [SortReducer::First, SortReducer::Last] {
                let query = Query {
                    collections: vec!["sources".into()],
                    filter: None,
                    fts: None,
                    options: QueryOptions {
                        sort: vec![SortField::computed(
                            SortExpression {
                                path: "links.title".into(),
                                binding: None,
                                reducer: Some(reducer),
                            },
                            false,
                        )],
                        limit: 12,
                        offset: 3,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                };
                for workers in [1, 2, 4, 8] {
                    let repo = crate::lifecycle::Index::build_repository_for_test(
                        &root,
                        vec![source_definition.clone(), target_definition.clone()],
                        RepositoryOptions {
                            query_workers: workers,
                            ..Default::default()
                        },
                        crate::source::test_support::fixture_registry(),
                    )
                    .map_err(|error| error.to_string())
                    .unwrap();
                    let actual = repo.search(&query).unwrap();
                    assert_eq!(actual.count, Some(20));
                    assert_eq!(actual.hits, repo.search_reference(&query).unwrap().hits);
                }
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn reference_forward_sort_reuses_sources_and_accounts_actual_reads() {
            fn source_definition(path: &str, relation: Option<&str>) -> CollectionDefinition {
                let mut definition = crate::source::test_support::document_folder(path, path);
                definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
                q4_field(&mut definition.schema, "id", LogicalType::Id);
                if let Some(target) = relation {
                    q4_field(&mut definition.schema, "entry", LogicalType::Id);
                    definition
                        .schema
                        .register_relation(RelationDefinition {
                            path: vec!["entry".into()],
                            target_collection: target.into(),
                            cardinality: RelationCardinality::One,
                            reverse_alias: None,
                        })
                        .unwrap();
                }
                definition
            }

            fn target_definition(path: &str) -> CollectionDefinition {
                let mut definition = crate::source::test_support::document_folder(path, path);
                definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
                q4_field(&mut definition.schema, "id", LogicalType::Id);
                q4_field(&mut definition.schema, "title", LogicalType::String);
                definition
            }

            fn sort_query(path: &str) -> Query {
                Query {
                    collections: vec!["roots".into()],
                    filter: None,
                    fts: None,
                    options: QueryOptions {
                        sort: vec![SortField::computed(
                            SortExpression {
                                path: path.into(),
                                binding: None,
                                reducer: None,
                            },
                            false,
                        )],
                        limit: 20,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                }
            }

            let one_hop_root =
                std::env::temp_dir().join(format!("objekte-forward-source-{}", std::process::id()));
            for directory in ["roots", "targets"] {
                std::fs::create_dir_all(one_hop_root.join(directory)).unwrap();
            }
            for (relative, document) in [
                ("roots/r1.row", r#"("id"="R1","entry"="T1")"#),
                ("roots/r2.row", r#"("id"="R2","entry"="T2")"#),
                ("targets/t1.row", r#"("id"="T1","title"="Alpha")"#),
                ("targets/t2.row", r#"("id"="T2","title"="Zulu")"#),
            ] {
                std::fs::write(one_hop_root.join(relative), document).unwrap();
            }
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &one_hop_root,
                vec![
                    source_definition("roots", Some("targets")),
                    target_definition("targets"),
                ],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let observations = crate::source::access::arm_search_read_observations();
            let result = repo.search_reference(&sort_query("entry.title")).unwrap();
            assert_eq!(hit_ids(&result), ["R1", "R2"], "{:?}", result.counters);
            assert_eq!(result.count, Some(2));
            assert_eq!(result.counters.relation_source_reads, 0);
            assert_eq!(result.counters.sort_target_reads, 2);
            assert_eq!(result.counters.files_opened, 4);
            assert_eq!(result.counters.record_parses, 4);
            let observations = observations.finish();
            assert_eq!(observations.len(), 2);
            assert!(observations.iter().all(|(path, _, _)| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('t'))
            }));
            let one_hop_bytes = [
                "roots/r1.row",
                "roots/r2.row",
                "targets/t1.row",
                "targets/t2.row",
            ]
            .into_iter()
            .map(|path| std::fs::metadata(one_hop_root.join(path)).unwrap().len())
            .sum::<u64>();
            assert_eq!(result.counters.bytes_read, one_hop_bytes);
            drop(repo);
            std::fs::remove_dir_all(one_hop_root).unwrap();

            let two_hop_root =
                std::env::temp_dir().join(format!("objekte-forward-source-two-{}", std::process::id()));
            for directory in ["roots", "intermediates", "targets"] {
                std::fs::create_dir_all(two_hop_root.join(directory)).unwrap();
            }
            for (relative, document) in [
                ("roots/r1.row", r#"("id"="R1","entry"="I")"#),
                ("roots/r2.row", r#"("id"="R2","entry"="I")"#),
                ("intermediates/i.row", r#"("id"="I","entry"="T")"#),
                ("targets/t.row", r#"("id"="T","title"="Tie")"#),
            ] {
                std::fs::write(two_hop_root.join(relative), document).unwrap();
            }
            let intermediate = source_definition("intermediates", Some("targets"));
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &two_hop_root,
                vec![
                    source_definition("roots", Some("intermediates")),
                    intermediate,
                    target_definition("targets"),
                ],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let observations = crate::source::access::arm_search_read_observations();
            let result = repo
                .search_reference(&sort_query("entry.entry.title"))
                .unwrap();
            assert_eq!(hit_ids(&result), ["R1", "R2"]);
            assert_eq!(result.count, Some(2));
            assert_eq!(result.counters.relation_source_reads, 1);
            assert_eq!(result.counters.sort_target_reads, 1);
            assert_eq!(result.counters.sort_target_reuse, 1);
            assert_eq!(result.counters.files_opened, 4);
            assert_eq!(result.counters.record_parses, 4);
            let observations = observations.finish();
            let observed_names = observations
                .iter()
                .map(|(path, _, _)| path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(observed_names, ["i.row", "t.row"]);
            let two_hop_bytes = [
                "roots/r1.row",
                "roots/r2.row",
                "intermediates/i.row",
                "targets/t.row",
            ]
            .into_iter()
            .map(|path| std::fs::metadata(two_hop_root.join(path)).unwrap().len())
            .sum::<u64>();
            assert_eq!(result.counters.bytes_read, two_hop_bytes);

            let intermediate_path = two_hop_root.join("intermediates/i.row");
            let target_path = two_hop_root.join("targets/t.row");
            let intermediate_bytes = std::fs::read(&intermediate_path).unwrap();
            let target_bytes = std::fs::read(&target_path).unwrap();
            std::fs::remove_file(&target_path).unwrap();
            let terminal_missing = repo
                .search_reference(&sort_query("entry.entry.title"))
                .unwrap();
            assert!(terminal_missing.hits.is_empty());
            assert_eq!(terminal_missing.count, None);
            assert!(!terminal_missing.counters.match_count_complete);
            assert!(
                terminal_missing
                    .counters
                    .source_diagnostics
                    .iter()
                    .any(|diagnostic| { diagnostic.file_path == std::path::Path::new("targets/t.row") })
            );
            std::fs::write(&target_path, &target_bytes).unwrap();

            std::fs::remove_file(&intermediate_path).unwrap();
            let intermediate_missing = repo
                .search_reference(&sort_query("entry.entry.title"))
                .unwrap();
            assert!(intermediate_missing.hits.is_empty());
            assert_eq!(intermediate_missing.count, None);
            assert!(!intermediate_missing.counters.match_count_complete);
            assert!(
                intermediate_missing
                    .counters
                    .source_diagnostics
                    .iter()
                    .any(|diagnostic| {
                        diagnostic.file_path == std::path::Path::new("intermediates/i.row")
                    })
            );
            std::fs::write(&intermediate_path, intermediate_bytes).unwrap();

            drop(repo);
            std::fs::remove_dir_all(two_hop_root).unwrap();
        }
    };
}
pub(super) use sort_binding_tests;

macro_rules! temporal_tests {
    () => {
        use super::*;

        #[test]
        fn calendar_and_offset_periods_use_written_precision() {
            let month = PeriodLiteral::parse("2024-02").unwrap();
            let days = month.for_type(&LogicalType::Date).unwrap();
            assert!(days.matches(&Value::Date(date("2024-02-29").unwrap()), Operator::Eq));
            assert!(!days.matches(&Value::Date(date("2024-03-01").unwrap()), Operator::Eq));
            assert!(days.matches(&Value::Date(date("2024-03-01").unwrap()), Operator::Gt));
            assert!(days.matches(&Value::Date(date("2024-02-01").unwrap()), Operator::Le));

            let minute = PeriodLiteral::parse("2026-11-06T14:30+01:00").unwrap();
            let utc = minute.for_type(&LogicalType::Datetime).unwrap();
            let during =
                Value::parse_typed_string("2026-11-06T13:30:59Z", &LogicalType::Datetime).unwrap();
            let after =
                Value::parse_typed_string("2026-11-06T13:31:00Z", &LogicalType::Datetime).unwrap();
            assert!(utc.matches(&during, Operator::Eq));
            assert!(utc.matches(&after, Operator::Gt));
            assert!(!utc.matches(&after, Operator::Eq));
            assert!(minute.for_type(&LogicalType::Date).is_err());

            let instant = PeriodLiteral::parse("2026-11-06T14:30:15.250+01:00").unwrap();
            assert_eq!(instant.end_ns - instant.start_ns, 1_000_000);
            assert!(PeriodLiteral::parse("2026-11-06T14:30:15.250Z").is_ok());
        }

        #[test]
        fn unix_seconds_round_up_fractional_and_negative_bounds() {
            let second = PeriodLiteral::parse("1969-12-31T23:59:59Z").unwrap();
            let bounds = second.for_type(&LogicalType::Unixtime).unwrap();
            assert!(bounds.matches(&Value::Unixtime(-1), Operator::Eq));
            assert!(!bounds.matches(&Value::Unixtime(0), Operator::Eq));
            let subsecond = PeriodLiteral::parse("1969-12-31T23:59:59.5Z").unwrap();
            let bounds = subsecond.for_type(&LogicalType::Unixtime).unwrap();
            assert_eq!(bounds.start, Value::Unixtime(0));
            assert_eq!(bounds.end, Value::Unixtime(0));
            assert!(!bounds.matches(&Value::Unixtime(-1), Operator::Eq));
            assert!(!bounds.matches(&Value::Unixtime(0), Operator::Eq));
            assert!(bounds.matches(&Value::Unixtime(0), Operator::Ne));
        }

        #[test]
        fn invalid_calendar_and_timestamp_forms_fail() {
            for text in [
                "2025-02-29",
                "2026-13",
                "0000-01",
                "2026-11-06T14:30",
                "2026-11-06T25:00Z",
                "2026-11-06T14:30:60Z",
                "2026-11-06T14:30:15.1234567890Z",
                "2026-11-06T14:30+24:00",
                "2026-11-0éT14:30Z",
            ] {
                assert!(PeriodLiteral::parse(text).is_err(), "{text}");
            }
        }
    };
}
pub(super) use temporal_tests;

macro_rules! validation_tests {
    () => {
        use std::time::{SystemTime, UNIX_EPOCH};

        use crate::model::LogicalType;
        use crate::query::options::ReturnMode;
        use crate::query::{Query, QueryOptions, parse_filter};
        use crate::repository::configuration::RepositoryOptions;
        use crate::schema::FieldDefinition;

        #[test]
        fn validates_binding_without_reopening_source_records() {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!("objekte-query-validation-{unique}"));
            std::fs::create_dir_all(root.join("data")).unwrap();
            let source = root.join("data/record.row");
            std::fs::write(&source, r#"("year"=1900)"#).unwrap();
            let mut definition = crate::source::test_support::document_folder("data", "data");
            definition
                .schema
                .register_field(FieldDefinition {
                    path: vec!["year".into()],
                    kind: LogicalType::Number,
                    required: true,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![definition],
                RepositoryOptions::default(),
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            std::fs::remove_file(&source).unwrap();
            let query = |filter: &str| Query {
                collections: vec!["data".into()],
                fts: None,
                filter: Some(parse_filter(filter).unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 1,
                    offset: 0,
                    count: false,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            };
            assert!(repo.validate_query(&query("year >= 1900")).is_ok());
            assert!(
                repo.validate_query(&query("year >= 'nineteen hundred'"))
                    .is_err()
            );
            let mut wrong_scope = query("year >= 1900");
            wrong_scope.collections = vec!["missing".into()];
            assert!(
                repo.validate_query(&wrong_scope)
                    .unwrap_err()
                    .contains("scope")
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    };
}
pub(super) use validation_tests;
