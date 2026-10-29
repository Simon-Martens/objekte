mod support;
use objekte_core::{CacheMode, Direction, Sort, ViewRequest};
use serde_json::json;
use support::*;

fn sort(field: &str, direction: Direction) -> Sort {
    Sort {
        field: field.into(),
        direction,
    }
}

#[test]
fn filter_hash_includes_values_operators_and_fields_but_not_and_order() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let c = r.model().collection("books").unwrap();
    let filters = ["score>=2", "status=draft"]
        .iter()
        .map(|s| c.parse_filter(s).unwrap())
        .collect::<Vec<_>>();
    let a = r
        .open_view(
            "books",
            &ViewRequest {
                filters: filters.clone(),
                ..all()
            },
        )
        .unwrap();
    let b = r
        .open_view(
            "books",
            &ViewRequest {
                filters: filters.iter().rev().cloned().collect(),
                ..all()
            },
        )
        .unwrap();
    assert_eq!(a.view_hashes, b.view_hashes);
    for changed in ["score>2", "score>=3", "count>=2"] {
        let filters = vec![
            r.model()
                .collection("books")
                .unwrap()
                .parse_filter(changed)
                .unwrap(),
            filters[1].clone(),
        ];
        let b = r
            .open_view("books", &ViewRequest { filters, ..all() })
            .unwrap();
        assert_ne!(a.view_hashes.filters, b.view_hashes.filters);
        assert_ne!(a.view_hashes.combined, b.view_hashes.combined);
        assert_eq!(a.view_hashes.sort, b.view_hashes.sort);
        assert_eq!(a.view_hashes.projection, b.view_hashes.projection);
    }
}

#[test]
fn resolved_defaults_and_explicit_equivalent_request_share_hashes() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let a = r.open_view("books", &ViewRequest::default()).unwrap();
    let c = r.model().collection("books").unwrap();
    let q = ViewRequest {
        sort: Some(c.list.default_sort.clone()),
        sub_sorts: Some(vec![Sort::default()]),
        columns: Some(c.list.columns.clone()),
        limit: Some(c.page_size),
        ..Default::default()
    };
    let b = r.open_view("books", &q).unwrap();
    assert_eq!(a.view_hashes, b.view_hashes);
    assert_eq!(a.rows, b.rows);
    let direct = f.repo(CacheMode::Disabled).open_view("books", &q).unwrap();
    assert_eq!(direct.view_hashes, b.view_hashes);
}

#[test]
fn subsort_directions_and_priority_change_order_and_hashes() {
    let f = Fixture::new();
    for (id, title, score) in [("a", "same", 1), ("b", "same", 3), ("c", "same", 2)] {
        f.edit(&format!("data/books/{id}.json"), |v| {
            v["title"] = json!(title);
            v["score"] = json!(score);
        });
    }
    let mut r = f.repo(CacheMode::Segments);
    let filter = r
        .model()
        .collection("books")
        .unwrap()
        .parse_filter("score>=1")
        .unwrap();
    let q = ViewRequest {
        sort: Some(sort("title", Direction::Asc)),
        sub_sorts: Some(vec![sort("score", Direction::Desc)]),
        filters: vec![filter],
        ..all()
    };
    let a = r.open_view("books", &q).unwrap();
    assert_eq!(
        a.rows,
        f.repo(CacheMode::Disabled)
            .open_view("books", &q)
            .unwrap()
            .rows
    );
    let tied = a
        .rows
        .iter()
        .filter(|r| ["a", "b", "c"].contains(&r.id.as_str()))
        .map(|r| r.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tied, vec!["b", "c", "a"]);
    let b = r
        .open_view(
            "books",
            &ViewRequest {
                sub_sorts: Some(vec![sort("score", Direction::Asc)]),
                ..q.clone()
            },
        )
        .unwrap();
    assert_ne!(a.view_hashes.sort, b.view_hashes.sort);
    let reversed = ViewRequest {
        sort: Some(sort("score", Direction::Desc)),
        sub_sorts: Some(vec![sort("title", Direction::Asc)]),
        ..q
    };
    let c = r.open_view("books", &reversed).unwrap();
    assert_ne!(a.view_hashes.sort, c.view_hashes.sort);
}

#[test]
fn mixed_directions_null_missing_and_secondary_id_order_match_direct() {
    let f = Fixture::new();
    for direction in [Direction::Asc, Direction::Desc] {
        for secondary in ["score", "title", "flag"] {
            for offset in [0, 2, 7] {
                let q = ViewRequest {
                    sort: Some(sort("status", direction)),
                    sub_sorts: Some(vec![
                        sort(secondary, Direction::Desc),
                        sort("@id", Direction::Desc),
                    ]),
                    offset,
                    ..all()
                };
                let segments = f.repo(CacheMode::Segments).open_view("books", &q).unwrap();
                let direct = f.repo(CacheMode::Disabled).open_view("books", &q).unwrap();
                assert_eq!(segments.rows, direct.rows);
                assert_eq!(segments.view_hashes, direct.view_hashes);
            }
        }
    }
}

#[test]
fn configured_subsort_changes_only_view_hash_when_fields_are_already_indexed() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    // Materialize the compound-view index before changing the configured order.
    // Permitted fields alone no longer imply that every index has been built.
    r.open_view(
        "books",
        &ViewRequest {
            sub_sorts: Some(vec![sort("title", Direction::Asc)]),
            ..all()
        },
    )
    .unwrap();
    let before = r.model().collection("books").unwrap().clone();
    f.edit(".editor/books/ui.json", |v| {
        v["list"]["subSorts"] = json!([{"field":"title","direction":"desc"}])
    });
    let actual = r.open_view("books", &all()).unwrap();
    let c = r.model().collection("books").unwrap();
    assert_ne!(c.view_hashes.sort, before.view_hashes.sort);
    assert_ne!(c.view_hashes.combined, before.view_hashes.combined);
    assert_eq!(c.view_hashes.filters, before.view_hashes.filters);
    assert_eq!(c.index_hash, before.index_hash);
    assert_eq!(actual.metrics.source_reads, actual.rows.len() as u64);
    assert_eq!(actual.metrics.parses, actual.rows.len() as u64);
    assert_eq!(actual.metrics.index_builds, 0);
    assert_eq!(
        actual.rows,
        f.repo(CacheMode::Disabled)
            .open_view("books", &all())
            .unwrap()
            .rows
    );
    // A new explicit primary order replaces the configured secondary order.
    let explicit = r
        .open_view(
            "books",
            &ViewRequest {
                sort: Some(sort("flag", Direction::Asc)),
                ..all()
            },
        )
        .unwrap();
    let empty = r
        .open_view(
            "books",
            &ViewRequest {
                sort: Some(sort("flag", Direction::Asc)),
                sub_sorts: Some(vec![]),
                ..all()
            },
        )
        .unwrap();
    assert_eq!(explicit.view_hashes, empty.view_hashes);
}

#[test]
fn invalid_subsorts_fail_before_source_io() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    for secondary in [
        vec![sort("score", Direction::Desc)],
        vec![sort("payload", Direction::Asc)],
        vec![sort("@id", Direction::Asc), sort("title", Direction::Asc)],
    ] {
        assert!(
            r.open_view(
                "books",
                &ViewRequest {
                    sub_sorts: Some(secondary),
                    ..all()
                }
            )
            .is_err()
        );
    }
    assert_eq!(r.metrics().source_reads, 0);
    assert!(!f.data.exists());
    for sub in [
        json!([{"field":"score","direction":"desc"}]),
        json!([{"field":"payload","direction":"asc"}]),
    ] {
        f.edit(".editor/books/ui.json", |v| v["list"]["subSorts"] = sub);
        assert!(
            objekte_core::Repository::load_repository(&f.root, f.options(CacheMode::Segments))
                .is_err()
        );
    }
}

#[test]
fn projection_pagination_and_count_are_part_of_resolved_view_fingerprint() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let a = r.open_view("books", &all()).unwrap();
    for q in [
        ViewRequest {
            columns: Some(vec!["title".into()]),
            ..all()
        },
        ViewRequest {
            limit: Some(1),
            ..all()
        },
        ViewRequest { offset: 1, ..all() },
        ViewRequest {
            count: false,
            ..all()
        },
    ] {
        let b = r.open_view("books", &q).unwrap();
        assert_ne!(a.view_hashes.combined, b.view_hashes.combined);
        assert_ne!(a.view_hashes.projection, b.view_hashes.projection);
        assert_eq!(a.view_hashes.filters, b.view_hashes.filters);
        assert_eq!(a.view_hashes.sort, b.view_hashes.sort);
    }
}

#[test]
fn schema_json_formatting_does_not_change_semantic_fingerprints() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    r.open_view("books", &all()).unwrap();
    let before = r.model().collection("books").unwrap().clone();
    let schema = f.read(".editor/books/schema.json");
    std::fs::write(
        f.root.join(".editor/books/schema.json"),
        serde_json::to_string_pretty(&schema).unwrap(),
    )
    .unwrap();
    let v = r.open_view("books", &all()).unwrap();
    let after = r.model().collection("books").unwrap();
    assert_eq!(before.schema_hash, after.schema_hash);
    assert_eq!(before.view_hashes, after.view_hashes);
    assert_eq!(v.metrics.source_reads, v.rows.len() as u64);
    assert_eq!(v.metrics.validations, v.rows.len() as u64);
}
