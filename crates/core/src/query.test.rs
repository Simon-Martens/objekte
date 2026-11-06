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
}

#[test]
fn every_fixed_filter_parses() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/musenalm_queries.json");
    for case in crate::workload::load_manifest(&path).unwrap() {
        Query::from_workload(&case).unwrap_or_else(|e| panic!("{}: {e}", case.id));
    }
}
