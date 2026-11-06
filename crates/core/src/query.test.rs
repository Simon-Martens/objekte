use super::*;

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
                descending: true
            },
            SortField {
                field: "year".into(),
                descending: false
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
        "SORT BY items[*].price",
        "SORT BY items[0].price",
        "SORT BY matrix[1][2]",
    ] {
        assert!(parse_structured_filter(input).is_err(), "{input}");
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
            descending: true
        }]
    );
    assert_eq!(query.options.limit, 3);
    assert_eq!(query.options.offset, 2);
    case.sort.push(SortField {
        field: "year".into(),
        descending: true,
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
