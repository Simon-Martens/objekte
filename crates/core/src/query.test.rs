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
    let case = WorkloadCase {
        fts_mode: FtsMode::Literal,
        fts_case_sensitivity: crate::query::CaseSensitivity::Sensitive,
        id: "Q000".into(),
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
    let query = Query::from_workload(&case).unwrap();
    assert_eq!(query.fts.unwrap().literal, "a && b");
    assert!(
        serde_json::to_value(&case)
            .unwrap()
            .get("fts_mode")
            .is_none()
    );
    let mut regex_case = case.clone();
    regex_case.fts_mode = FtsMode::Regex(RegexOptions {
        multi_line: true,
        dot_matches_new_line: false,
    });
    assert_eq!(
        Query::from_workload(&regex_case).unwrap().fts.unwrap().mode,
        regex_case.fts_mode
    );
    assert!(
        serde_json::to_value(&case)
            .unwrap()
            .get("fts_case_sensitivity")
            .is_none()
    );
    let mut insensitive = case.clone();
    insensitive.fts_case_sensitivity = CaseSensitivity::Insensitive;
    assert_eq!(
        Query::from_workload(&insensitive)
            .unwrap()
            .fts
            .unwrap()
            .case_sensitivity,
        CaseSensitivity::Insensitive
    );
}

#[test]
fn workload_fts_binding_colon_stays_outside_quoted_path_properties() {
    let case = WorkloadCase {
        id: "quoted-fts-fields".into(),
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
    let fields = Query::from_workload(&case).unwrap().fts.unwrap().fields;
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
    use crate::reference::FieldDefinition;
    use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
    use crate::source::SourceFormat;

    let root = std::env::temp_dir().join(format!(
        "objekte-colon-path-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.json"), r#"{"a:b":"Zulu"}"#).unwrap();
    std::fs::write(root.join("data/two.json"), r#"{"a:b":"Alpha"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
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
    let repo = Repository::open_with_options(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let mut case = WorkloadCase {
        id: "colon-path".into(),
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
        ids(&repo, &Query::from_workload(&case).unwrap()),
        ["path:two.json"]
    );

    case.filter = None;
    case.fts = Some("Alpha".into());
    case.fts_fields = vec!["[\"a:b\"]".into()];
    assert_eq!(
        ids(&repo, &Query::from_workload(&case).unwrap()),
        ["path:two.json"]
    );

    case.fts = None;
    case.fts_fields.clear();
    case.filter = Some("SORT BY [\"a:b\"]".into());
    assert_eq!(
        ids(&repo, &Query::from_workload(&case).unwrap()),
        ["path:two.json", "path:one.json"]
    );
    drop(repo);
    let restored = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        ids(&restored, &Query::from_workload(&case).unwrap()),
        ["path:two.json", "path:one.json"]
    );
    drop(restored);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_fixed_filter_parses() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    for case in crate::workload::load_manifest(&path).unwrap() {
        Query::from_workload(&case).unwrap_or_else(|e| panic!("{}: {e}", case.id));
    }
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
    let mut case = WorkloadCase {
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        id: "inline-sort".into(),
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
    let query = Query::from_workload(&case).unwrap();
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
    assert!(
        Query::from_workload(&case)
            .unwrap_err()
            .contains("conflicts")
    );
}

#[test]
fn complete_regex_pattern_parameter_binds_once_without_interpolation() {
    let mut case = WorkloadCase {
        id: "regex-pattern-parameter".into(),
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
    let template = Query::from_workload(&case).unwrap();
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
        Query::from_workload(&case)
            .unwrap()
            .fts
            .unwrap()
            .pattern_parameter
            .is_none()
    );
    case.fts_mode = FtsMode::Regex(RegexOptions::default());
    case.fts = Some("prefix$pattern".into());
    assert!(
        Query::from_workload(&case)
            .unwrap()
            .fts
            .unwrap()
            .pattern_parameter
            .is_none()
    );
}
