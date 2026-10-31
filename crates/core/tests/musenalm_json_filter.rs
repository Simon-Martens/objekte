use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository};
use std::{path::PathBuf, time::Instant};

#[test]
#[ignore = "compares global JSON filters across all Musenalm records; run with --release --ignored --nocapture"]
fn global_filters_match_source_execution_on_musenalm() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut options = Options::new();
    options.cache = CacheMode::Memory;
    let mut memory = Repository::load_repository(&root, options.clone()).unwrap();
    options.cache = CacheMode::Disabled;
    let mut direct = Repository::load_repository(&root, options).unwrap();
    for (collection, expression) in [
        (None, "year >= 1800 && year < 1805"),
        (None, "preferred_title ~ :ignorecase('liebe')"),
        (
            None,
            "contributors.agents ?= '1500' || entry = '00fx8ln651na0e3'",
        ),
        (Some("contents"), "year >= 1800 && year < 1805"),
    ] {
        let request = JsonQuery {
            filter: expression.into(),
            collections: collection.map(|c| vec![c.into()]),
            limit: 200000,
            count: true,
            backend: JsonQueryBackend::Direct,
            ..Default::default()
        };
        let start = Instant::now();
        let source = direct.query_json(&request).unwrap();
        let direct_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        let cached = memory
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Bigram,
                ..request.clone()
            })
            .unwrap();
        let bigram_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(source.total, cached.total, "{expression}");
        assert_eq!(
            source.records.len(),
            source.total.unwrap().min(request.limit),
            "{expression}"
        );
        assert_eq!(
            cached.records.len(),
            cached.total.unwrap().min(request.limit),
            "{expression}"
        );
        for (a, b) in source.records.iter().zip(&cached.records) {
            assert_eq!(
                (&a.collection, &a.id, &a.path, &a.data),
                (&b.collection, &b.id, &b.path, &b.data),
                "{expression}"
            );
        }
        let start = Instant::now();
        let warm = memory
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Bigram,
                ..request.clone()
            })
            .unwrap();
        let warm_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(warm.total, cached.total);
        assert_eq!(
            warm.records.len(),
            warm.total.unwrap().min(request.limit),
            "{expression}"
        );
        let json_index_used = warm.query_plan.iter().any(|p| p.contains("bigram"));
        if collection.is_some() {
            let page_size = 2048;
            let mut offset = 0;
            while offset < source.records.len() {
                let page = memory
                    .query_json(&JsonQuery {
                        limit: page_size,
                        offset,
                        count: false,
                        backend: JsonQueryBackend::Bigram,
                        ..request.clone()
                    })
                    .unwrap();
                let expected =
                    &source.records[offset..(offset + page_size).min(source.records.len())];
                assert_eq!(page.records.len(), expected.len(), "page at {offset}");
                for (actual, expected) in page.records.iter().zip(expected) {
                    assert_eq!(
                        (&actual.collection, &actual.id, &actual.path, &actual.data),
                        (
                            &expected.collection,
                            &expected.id,
                            &expected.path,
                            &expected.data
                        ),
                        "page at {offset}"
                    );
                }
                offset += page.records.len();
            }
            assert_eq!(offset, source.total.unwrap());
            assert!(
                memory
                    .query_json(&JsonQuery {
                        limit: page_size,
                        offset,
                        count: false,
                        backend: JsonQueryBackend::Bigram,
                        ..request.clone()
                    })
                    .unwrap()
                    .records
                    .is_empty()
            );
        }
        assert_eq!(warm.metrics.enumerations, 0);
        println!(
            "{}",
            serde_json::json!({"collection":collection,"filter":expression,"matches":cached.total,"returned":cached.records.len(),"direct_ms":direct_ms,"bigram_ms":bigram_ms,"warm_bigram_ms":warm_ms,"bigram_query_ms":cached.metrics.query_nanos as f64/1e6,"source_reads":cached.metrics.source_reads,"bigram_prefilter_used":json_index_used})
        );
    }
}
