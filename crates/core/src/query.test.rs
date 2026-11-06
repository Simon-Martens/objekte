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
    assert!(matches!(term.operand, Operand::Field(FieldExpression {
        path: FieldPath { root: crate::model::PathRoot::Data, segments },
        modifiers,
        span: (14, 35),
    }) if segments == ["alternate_title"] && modifiers == [Modifier::Lower]));

    let FilterExpr::Term(term) = parse_filter("year >= $minimum_year").unwrap() else {
        panic!("expected term");
    };
    assert!(matches!(term.operand, Operand::Parameter(name) if name == "minimum_year"));
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
