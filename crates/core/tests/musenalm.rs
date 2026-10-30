//! Full real-data acceptance, deliberately opt-in for routine test runs.
use objekte_core::{
    CacheMode, Direction, Filter, Freshness, Operator, Options, Repository, Sort, ValueType,
    ViewRequest,
};
use serde_json::json;
use std::{path::PathBuf, time::Instant};
#[test]
#[ignore = "reads all 148,486 example records; run with --release --ignored --nocapture"]
fn all_musenalm_collections_match_direct_execution() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut memory_options = Options::new();
    memory_options.cache = CacheMode::Memory;
    let mut cached = Repository::load_repository(&root, memory_options.clone()).unwrap();
    let mut disabled = Options::new();
    disabled.cache = CacheMode::Disabled;
    let mut direct = Repository::load_repository(&root, disabled).unwrap();
    assert_eq!(cached.model().collections.len(), 7);
    assert_eq!(cached.model().graph.edges.len(), 6);
    assert!(cached.model().graph.cycles.is_empty());
    let counts = [
        ("agents", 4010),
        ("contents", 133948),
        ("entries", 4928),
        ("items", 4180),
        ("pages", 8),
        ("places", 210),
        ("series", 1202),
    ];
    let mut report = Vec::new();
    for (name, count) in counts {
        let q = ViewRequest {
            limit: Some(200000),
            count: true,
            ..Default::default()
        };
        let now = Instant::now();
        let source = direct.open_view(name, &q).unwrap();
        let source_ms = now.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(source.rows.len(), count);
        let now = Instant::now();
        let indexed = cached.open_view(name, &q).unwrap();
        let cold_ms = now.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(indexed.rows, source.rows, "{name}");
        assert_eq!(indexed.total, Some(count));
        cached.validate_collection(name).unwrap();
        let validated = cached.open_view(name, &q).unwrap();
        assert!(
            validated
                .states
                .values()
                .all(|s| s.status == objekte_core::RecordStatus::Valid),
            "{name} validation"
        );
        let now = Instant::now();
        let warm = cached.open_view(name, &ViewRequest::default()).unwrap();
        let warm_ms = now.elapsed().as_secs_f64() * 1000.0;
        assert!(!warm.query_plan.is_empty());
        assert_eq!(
            warm.rows,
            source.rows.iter().take(50).cloned().collect::<Vec<_>>()
        );
        let numbers = cached.model().collections[name]
            .indexes
            .iter()
            .filter(|i| {
                matches!(i.kind, ValueType::Number | ValueType::Integer)
                    && cached.model().collections[name]
                        .list
                        .sortable
                        .as_ref()
                        .unwrap()
                        .contains(&i.name)
            })
            .map(|i| i.name.clone())
            .collect::<Vec<_>>();
        for field in numbers {
            let q = ViewRequest {
                sort: Some(Sort {
                    field: field.clone(),
                    direction: Direction::Desc,
                }),
                limit: Some(100),
                offset: 1,
                count: true,
                ..Default::default()
            };
            let a = direct.open_view(name, &q).unwrap();
            let b = cached.open_view(name, &q).unwrap();
            assert_eq!(a.rows, b.rows, "{name} numeric sort {field}");
            assert_eq!(a.total, b.total);
        }
        let filters = cached.model().collections[name].list.filters.clone();
        for field in filters {
            let kind = cached.model().collections[name].fields[&field].kind;
            let value = if matches!(kind, ValueType::Number | ValueType::Integer) {
                json!(1800)
            } else {
                json!("")
            };
            let q = ViewRequest {
                filters: vec![Filter {
                    field,
                    operator: if kind == ValueType::Number {
                        Operator::Ge
                    } else {
                        Operator::Eq
                    },
                    value,
                }],
                limit: Some(200000),
                count: true,
                ..Default::default()
            };
            let a = direct.open_view(name, &q).unwrap();
            let b = cached.open_view(name, &q).unwrap();
            assert_eq!(a.rows, b.rows, "{name} filter");
            assert_eq!(a.total, b.total);
        }
        let item = json!({"collection":name,"records":count,"direct_full_ms":source_ms,"cold_memory_full_ms":cold_ms,"warm_page_ms":warm_ms,"build":indexed.metrics});
        println!("{item}");
        report.push(item);
    }
    let mut metadata = Repository::load_repository(&root, memory_options).unwrap();
    for (name, count) in counts {
        let view = metadata
            .open_view(
                name,
                &ViewRequest {
                    count: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(view.total, Some(count));
        assert_eq!(view.metrics.enumerations, 1);

        assert_eq!(
            view.rows,
            cached
                .open_view(name, &ViewRequest::default())
                .unwrap()
                .rows
        );
    }
    let compound = ViewRequest {
        sort: Some(Sort {
            field: "year".into(),
            direction: Direction::Asc,
        }),
        sub_sorts: Some(vec![Sort {
            field: "numbering".into(),
            direction: Direction::Desc,
        }]),
        limit: Some(100),
        ..Default::default()
    };
    assert_eq!(
        direct.open_view("contents", &compound).unwrap().rows,
        cached.open_view("contents", &compound).unwrap().rows
    );
    let mut strict = Options::new();
    strict.cache = CacheMode::Memory;
    strict.freshness = Freshness::HashOnRequest;
    let mut reopened = Repository::load_repository(&root, strict).unwrap();
    for (name, count) in counts {
        let v = reopened
            .open_view(
                name,
                &ViewRequest {
                    count: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(v.total, Some(count));
    }
    if let Ok(path) = std::env::var("OBJEKTE_ACCEPTANCE_REPORT") {
        std::fs::write(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(path),
            serde_json::to_vec_pretty(
                &json!({"collections":report,"metadata_reopen_verified":true,"strict_reopen_verified":true,"compound_sort_verified":true}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}
