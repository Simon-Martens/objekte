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
