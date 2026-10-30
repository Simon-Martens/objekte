//! Complete match-set comparison for the Musenalm query workload.
use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository};
use serde::Deserialize;
use std::path::PathBuf;


/// One filter and its expected source-derived result set.
#[derive(Deserialize)]
struct Case {
    name: String,
    filter: String,
}


#[test]
#[ignore = "reads the 133,948 Musenalm content records across the complete query workload; run with --release --ignored --nocapture"]
fn every_bigram_match_agrees_with_direct_source_evaluation() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut options = Options::new();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::load_repository(&root, options.clone()).unwrap();
    let mut bigram = Repository::load_repository(&root, options).unwrap();
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../bench/workloads/musenalm-bigram.json")).unwrap();
    for case in cases {
        let query = JsonQuery {
            filter: case.filter,
            collections: Some(vec!["contents".into()]),
            limit: 200_000,
            count: true,
            backend: JsonQueryBackend::Direct,
            ..Default::default()
        };
        let expected = direct.query_json(&query).unwrap();
        let expected_total = expected.total;
        let expected_ids: Vec<_> = expected
            .records
            .into_iter()
            .map(|record| record.id)
            .collect();
        let actual = bigram
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Bigram,
                ..query
            })
            .unwrap();
        assert_eq!(actual.total, expected_total, "{} total", case.name);
        assert_eq!(
            actual.records.len(),
            expected_ids.len(),
            "{} count",
            case.name
        );
        for (actual, expected) in actual.records.iter().zip(&expected_ids) {
            assert_eq!(&actual.id, expected, "{} identity", case.name);
        }
        println!("{}: {} exact matches", case.name, expected_ids.len());
    }
}
