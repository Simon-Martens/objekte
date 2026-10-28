use objekte_core::{CacheMode, JsonQuery, Options, Repository, ValidationMode};
use std::{path::PathBuf, time::Instant};

#[test]
#[ignore = "compares global JSON filters across all Musenalm records; run with --release --ignored --nocapture"]
fn global_filters_match_source_execution_on_musenalm() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let tmp = tempfile::tempdir().unwrap();
    let mut options = Options::new(tmp.path());
    options.ingest_workers = 4;
    options.validation = ValidationMode::Manual;
    let mut sql = Repository::load_repository(&root, options.clone()).unwrap();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::load_repository(&root, options).unwrap();
    for expression in [
        "year >= 1800 && year < 1805",
        "preferred_title ~ 'liebe'",
        "contributors.agents ?= '1500' || exists(contents[0])",
    ] {
        let request = JsonQuery {
            filter: expression.into(),
            limit: 200000,
            count: true,
            ..Default::default()
        };
        let start = Instant::now();
        let source = direct.query_json(&request).unwrap();
        let direct_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let cached = sql.query_json(&request).unwrap();
        let sql_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(source.total, cached.total, "{expression}");
        assert_eq!(source.total, Some(source.records.len()), "{expression}");
        assert_eq!(cached.total, Some(cached.records.len()), "{expression}");
        assert!(cached.total.unwrap() > 0);
        for (a, b) in source.records.iter().zip(&cached.records) {
            assert_eq!(
                (&a.collection, &a.id, &a.path, &a.data),
                (&b.collection, &b.id, &b.path, &b.data),
                "{expression}"
            );
        }
        let start = Instant::now();
        let warm = sql.query_json(&request).unwrap();
        let warm_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(warm.total, cached.total);
        assert_eq!(warm.total, Some(warm.records.len()), "{expression}");
        assert_eq!(warm.metrics.source_reads, 0);
        assert_eq!(warm.metrics.enumerations, 0);
        println!(
            "{}",
            serde_json::json!({"filter":expression,"matches":cached.total,"direct_ms":direct_ms,"sqlite_ms":sql_ms,"warm_sqlite_ms":warm_ms,"sql_query_ms":cached.metrics.query_nanos as f64/1e6,"source_reads":cached.metrics.source_reads})
        );
    }
}
