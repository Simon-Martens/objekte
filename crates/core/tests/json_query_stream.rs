//! Streaming query parity and provisional-result behavior.

mod support;

use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Repository};
use serde_json::json;
use std::fs;
use support::Fixture;


fn backend_repository(fixture: &Fixture, _backend: JsonQueryBackend) -> Repository {
    fixture.repo(CacheMode::Memory)
}

#[test]
fn streamed_pages_match_collected_queries_for_every_backend() {
    let fixture = Fixture::new();
    for backend in [
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        for filter in [
            "score >= 2",
            "title ~ 'a'",
            "title = 'Alpha'",
            "score < 2 || title = 'Beta'",
            "exists(payload)",
        ] {
            for count in [false, true] {
                let request = JsonQuery {
                    filter: filter.into(),
                    collections: Some(vec!["books".into()]),
                    backend,
                    count,
                    offset: 1,
                    limit: 2,
                    ..Default::default()
                };
                let mut repository = backend_repository(&fixture, backend);
                let collected = repository.query_json(&request).unwrap();
                let mut streamed = Vec::new();
                let summary = repository
                    .query_json_stream(&request, |ordinal, record| {
                        streamed.push((ordinal, record.clone()));
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(summary.returned, collected.records.len(), "{backend:?}");
                assert_eq!(summary.total, collected.total, "{backend:?}");
                assert_eq!(
                    summary.dependencies.collections,
                    collected.dependencies.collections
                );
                assert_eq!(summary.filter_hash, collected.filter_hash);
                assert_eq!(summary.query_plan, collected.query_plan);
                assert_eq!(
                    streamed
                        .iter()
                        .map(|(ordinal, _)| *ordinal)
                        .collect::<Vec<_>>(),
                    (1..=streamed.len()).collect::<Vec<_>>()
                );
                assert_eq!(
                    serde_json::to_value(
                        streamed
                            .into_iter()
                            .map(|(_, record)| record)
                            .collect::<Vec<_>>()
                    )
                    .unwrap(),
                    serde_json::to_value(collected.records).unwrap(),
                    "{backend:?}"
                );
            }
        }
    }
}

#[test]
fn a_late_scalar_error_invalidates_earlier_streamed_records() {
    let fixture = Fixture::new();
    let books = fixture.root.join("data/books");
    for entry in fs::read_dir(&books).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for number in 0..600 {
        fs::write(
            books.join(format!("r{number:03}.json")),
            serde_json::to_vec(&json!({"title":"hit","payload":null})).unwrap(),
        )
        .unwrap();
    }
    fs::write(
        books.join("zbad.json"),
        serde_json::to_vec(&json!({"title":["hit","other"],"payload":null})).unwrap(),
    )
    .unwrap();
    for backend in [
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        let mut repository = backend_repository(&fixture, backend);
        let request = JsonQuery {
            filter: "title = 'hit'".into(),
            collections: Some(vec!["books".into()]),
            backend,
            count: true,
            limit: 2,
            ..Default::default()
        };
        let mut seen = Vec::new();
        let error = repository
            .query_json_stream(&request, |ordinal, record| {
                seen.push((ordinal, record.id.clone()));
                Ok(())
            })
            .unwrap_err();
        assert_eq!(
            seen,
            [(1, "r000".into()), (2, "r001".into())],
            "{backend:?}"
        );
        assert!(error.to_string().contains("multiple values"), "{backend:?}");
    }
}
