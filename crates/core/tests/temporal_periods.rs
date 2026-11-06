use std::collections::HashMap;

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

#[test]
fn typed_periods_preserve_half_open_scalar_and_array_answers() {
    let root = std::env::temp_dir().join(format!(
        "objekte-temporal-periods-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        (
            "A",
            r#"{"id":"A","issued":"2024-02-29","created":"2026-11-06T13:30:59Z","epoch":-1,"dates":["2024-03-01","2024-01-01"],"declared_size":1024}"#,
        ),
        (
            "B",
            r#"{"id":"B","issued":"2024-03-01","created":"2026-11-06T13:31:00Z","epoch":0,"dates":["2024-02-29","2024-03-01"],"declared_size":1536}"#,
        ),
        (
            "C",
            r#"{"id":"C","issued":"2024-02-01","created":"2026-11-06T13:30:00Z","epoch":1,"dates":[],"declared_size":2500}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind, multi) in [
        ("id", LogicalType::String, false),
        ("issued", LogicalType::Date, false),
        ("created", LogicalType::Datetime, false),
        ("epoch", LogicalType::Unixtime, false),
        ("declared_size", LogicalType::Number, false),
        (
            "dates",
            LogicalType::Array(Box::new(LogicalType::Date)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: HashMap::new(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("issued = 2024-02", vec!["A", "C"]),
        ("issued != 2024-02", vec!["B"]),
        ("issued < 2024-02", vec![]),
        ("issued <= 2024-02", vec!["A", "C"]),
        ("issued > 2024-02", vec!["B"]),
        ("issued >= 2024-02", vec!["A", "B", "C"]),
        ("created = 2026-11-06T14:30+01:00", vec!["A", "C"]),
        ("created >= 2026-11-06T14:30:15.250+01:00", vec!["A", "B"]),
        ("epoch = 1969-12-31T23:59:59Z", vec!["A"]),
        ("epoch = 1969-12-31T23:59:59.5Z", vec![]),
        ("dates ?= 2024-02", vec!["B"]),
        ("dates = 2024-02", vec!["C"]),
        ("dates ?!= 2024-02", vec!["A", "B"]),
    ] {
        let request = query(filter);
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        let ids = actual
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected, "{filter}");
        assert_eq!(actual.count, reference.count, "{filter}");
        assert_eq!(actual.hits, reference.hits, "{filter}");
        if filter.starts_with("issued") && filter != "issued < 2024-02" {
            assert_eq!(actual.counters.files_opened, 0, "{filter}");
        }
    }
    assert!(repo.search(&query("issued = 2026-11-06T14:30Z")).is_err());
    assert!(repo.search(&query("epoch = 2024-02")).is_ok());
    assert!(repo.search(&query("issued ~ 2024-02")).is_err());
    assert!(parse_filter("issued = 2024-02-30").is_err());
    assert!(matches!(
        parse_filter("issued = 2024").unwrap(),
        objekte_search_core::query::FilterExpr::Term(_)
    ));
    for (filter, expected) in [
        ("declared_size = 1KiB", vec!["A"]),
        ("declared_size = 1536B", vec!["B"]),
        ("declared_size = 1.5KiB", vec!["B"]),
        ("declared_size = 2.5KB", vec!["C"]),
        (
            "declared_size >= 1KiB && declared_size < 2.5KB",
            vec!["A", "B"],
        ),
        ("declared_size > 1MiB", vec![]),
        ("@file.size < 1KiB", vec!["A", "B", "C"]),
        ("@record.byte_length < 2.5KB", vec!["A", "B", "C"]),
    ] {
        let request = query(filter);
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(actual.hits, reference.hits, "{filter}");
        assert_eq!(actual.count, reference.count, "{filter}");
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{filter}"
        );
    }
    for invalid in [
        "0.1B",
        "-1MiB",
        "1mib",
        "1PB",
        "170141183460469231731687303715884105728B",
    ] {
        assert!(
            parse_filter(&format!("declared_size = {invalid}")).is_err(),
            "{invalid}"
        );
    }
    let equivalent_size = repo.search(&query("declared_size = 1.500KiB")).unwrap();
    assert!(equivalent_size.counters.verified_result_hit);
    assert_eq!(equivalent_size.hits[0].record_id, "B");
    let today = query("issued = @today");
    let february = time::OffsetDateTime::parse(
        "2024-02-29T23:59:59Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let march = time::OffsetDateTime::parse(
        "2024-03-01T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let before = today.with_clock(february).unwrap();
    let after = today.with_clock(march).unwrap();
    for (request, expected) in [
        (&before, vec!["A"]),
        (&after, vec!["B"]),
        (&before, vec!["A"]),
    ] {
        let actual = repo.search(request).unwrap();
        let reference = repo.search_reference(request).unwrap();
        assert_eq!(actual.hits, reference.hits);
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
    assert!(repo.search(&before).unwrap().counters.verified_result_hit);
    let month_query = query("issued = @thisMonth").with_clock(february).unwrap();
    assert_eq!(
        repo.search(&month_query)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "C"]
    );
    let mut next_page = month_query.clone();
    next_page.options.limit = 1;
    next_page.options.offset = 1;
    assert_eq!(repo.search(&next_page).unwrap().hits[0].record_id, "C");
    let date_end = query("issued <= @todayEnd").with_clock(february).unwrap();
    assert_eq!(
        repo.search(&date_end)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "C"]
    );
    assert!(
        repo.search(&query("issued = @todayEnd").with_clock(february).unwrap())
            .unwrap()
            .hits
            .is_empty()
    );
    let instant = time::OffsetDateTime::parse(
        "2026-11-06T13:30:59Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let now_query = query("created = @now").with_clock(instant).unwrap();
    let now_matches = repo.search(&now_query).unwrap();
    assert_eq!(
        now_matches
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
    assert_eq!(
        now_matches.hits,
        repo.search_reference(&now_query).unwrap().hits
    );
    let half_second = time::OffsetDateTime::parse(
        "1969-12-31T23:59:59.5Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let unix_query = query("epoch >= @now").with_clock(half_second).unwrap();
    let unix_matches = repo.search(&unix_query).unwrap();
    assert_eq!(
        unix_matches
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["B", "C"]
    );
    assert_eq!(
        unix_matches.hits,
        repo.search_reference(&unix_query).unwrap().hits
    );
    let last_ns = query("epoch <= @todayEnd").with_clock(half_second).unwrap();
    let before_midnight = repo.search(&last_ns).unwrap();
    assert_eq!(
        before_midnight
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
    assert_eq!(
        before_midnight.hits,
        repo.search_reference(&last_ns).unwrap().hits
    );
    assert!(
        repo.search(&query("issued = @unknown"))
            .unwrap_err()
            .contains("unknown time macro")
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}
