mod support;
use objekte_core::{CacheMode, Direction, Filter, Operator, Sort, ViewRequest};
use serde_json::{Value, json};
use support::*;

fn parity(f: &Fixture, request: &ViewRequest) -> objekte_core::ViewResult {
    let direct = f
        .repo(CacheMode::Disabled)
        .open_view("books", request)
        .unwrap();
    let segments = f
        .repo(CacheMode::Segments)
        .open_view("books", request)
        .unwrap();
    let memory = f
        .repo(CacheMode::Memory)
        .open_view("books", request)
        .unwrap();
    assert_eq!(segments.rows, direct.rows);
    assert_eq!(segments.total, direct.total);
    assert_eq!(segments.columns, direct.columns);
    assert_eq!(memory.rows, direct.rows);
    assert_eq!(memory.total, direct.total);
    assert_eq!(memory.columns, direct.columns);
    assert_eq!(memory.states, segments.states);
    segments
}

#[test]
fn exact_numeric_order_null_missing_and_tie_breaks() {
    let f = Fixture::new();
    let view = parity(&f, &all());
    assert_eq!(ids(&view.rows), vec!["f", "b", "a", "g", "c", "d", "e"]);
    assert_eq!(view.total, Some(7));
    assert!(!view.rows.last().unwrap().values.contains_key("score"));
    assert!(view.rows[5].values["score"].is_null());
    let desc = ViewRequest {
        sort: Some(Sort {
            field: "score".into(),
            direction: Direction::Desc,
        }),
        ..all()
    };
    assert_eq!(
        ids(&parity(&f, &desc).rows),
        vec!["c", "a", "g", "b", "f", "d", "e"]
    );
}

#[test]
fn all_sortable_fields_directions_and_pagination_match() {
    let f = Fixture::new();
    let fields = f.repo(CacheMode::Disabled).model().collections["books"]
        .list
        .sortable
        .clone()
        .unwrap();
    for field in fields.into_iter().chain(std::iter::once("@id".into())) {
        for direction in [Direction::Asc, Direction::Desc] {
            for offset in [0, 2, 7, 30] {
                parity(
                    &f,
                    &ViewRequest {
                        sort: Some(Sort {
                            field: field.clone(),
                            direction,
                        }),
                        offset,
                        limit: Some(2),
                        count: true,
                        ..Default::default()
                    },
                );
            }
        }
    }
}

#[test]
fn typed_filters_include_empty_and_null_results() {
    let f = Fixture::new();
    let r = f.repo(CacheMode::Disabled);
    let c = &r.model().collections["books"];
    for text in [
        "score=2",
        "score>2",
        "score>=2",
        "score<2",
        "score<=2",
        "score=null",
        "status=draft",
        "status=\"reviewed\"",
        "status=null",
        "status=absent",
        "flag=true",
        "flag=false",
        "flag=null",
        "@id=a",
    ] {
        let filter = c.parse_filter(text).unwrap();
        let view = parity(
            &f,
            &ViewRequest {
                filters: vec![filter],
                ..all()
            },
        );
        if text == "score=2" {
            assert_eq!(ids(&view.rows), vec!["a", "g"])
        }
        if text == "score=null" {
            assert_eq!(ids(&view.rows), vec!["d"])
        }
        if text == "status=absent" {
            assert_eq!(view.total, Some(0))
        }
    }
    let filters = ["status=draft", "score>=2", "score<10"]
        .iter()
        .map(|s| c.parse_filter(s).unwrap())
        .collect();
    assert_eq!(
        ids(&parity(&f, &ViewRequest { filters, ..all() }).rows),
        vec!["a", "g"]
    );
}

#[test]
fn projection_retains_identity_labels_and_json_types() {
    let f = Fixture::new();
    let v = parity(
        &f,
        &ViewRequest {
            columns: Some(vec!["@id".into(), "flag".into(), "quote\"slash/".into()]),
            ..all()
        },
    );
    let a = v.rows.iter().find(|r| r.id == "a").unwrap();
    assert_eq!(a.label, "Beta");
    assert_eq!(a.collection, "books");
    assert_eq!(a.path, "data/books/a.json");
    assert_eq!(a.values["flag"], json!(true));
    assert_eq!(a.values["@id"], "a");
    assert_eq!(a.values.len(), 3);
    assert_eq!(v.rows.iter().find(|r| r.id == "d").unwrap().label, "d");
    assert_eq!(v.rows.iter().find(|r| r.id == "e").unwrap().label, "e");
    assert_eq!(v.rows.iter().find(|r| r.id == "f").unwrap().label, "");
}

#[test]
fn warm_segment_views_read_only_the_requested_page_and_collection() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let first = r.open_view("books", &all()).unwrap();
    assert_eq!(first.metrics.source_reads, 14);
    assert_eq!(first.metrics.parses, 14);
    assert_eq!(first.metrics.rows_inserted, 7);
    assert!(first.metrics.index_builds > 0);
    let warm = r.open_view("books", &all()).unwrap();
    assert_eq!(warm.rows, first.rows);
    assert_eq!(warm.metrics.source_reads, 7);
    assert_eq!(warm.metrics.enumerations, 0);
    assert_eq!(warm.metrics.parses, 7);
    assert!(
        warm.query_plan
            .iter()
            .any(|s| s.contains("ordered segment index"))
    );
    let status = r.cache_status().unwrap();
    assert!(
        status
            .collections
            .iter()
            .filter(|c| c.collection != "books")
            .all(|c| c.state == "absent")
    );
}

#[test]
fn warm_memory_views_read_only_the_requested_page() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Memory);
    let request = ViewRequest {
        limit: Some(2),
        count: true,
        filters: vec![Filter {
            field: "score".into(),
            operator: Operator::Ge,
            value: json!(2),
        }],
        ..Default::default()
    };
    let first = repo.open_view("books", &request).unwrap();
    let warm = repo.open_view("books", &request).unwrap();
    assert_eq!(first.rows, warm.rows);
    assert_eq!(warm.total, Some(3));
    assert_eq!(warm.metrics.source_reads, 2);
    assert_eq!(warm.metrics.bigram_open_nanos, 0);
    assert!(warm.query_plan[0].contains("ordered in-memory index"));
    assert!(!f.data.join("segments").exists());
}

#[test]
fn memory_revisions_and_ordered_keys_follow_edits_and_deletions() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Memory);
    repo.open_view("books", &all()).unwrap();
    let before = repo.record_revision("books", "a").unwrap().unwrap();
    f.edit("data/books/a.json", |value| value["score"] = json!(-42));
    repo.refresh("books").unwrap();
    let after = repo.record_revision("books", "a").unwrap().unwrap();
    assert!(after.revision > before.revision);
    assert!(after.changed_at_nanos >= before.changed_at_nanos);
    assert_eq!(repo.open_view("books", &all()).unwrap().rows[0].id, "a");
    std::fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    repo.refresh("books").unwrap();
    let deleted = repo.record_revision("books", "a").unwrap().unwrap();
    assert!(!deleted.present);
    assert!(deleted.revision > after.revision);
    assert!(
        !repo
            .open_view("books", &all())
            .unwrap()
            .rows
            .iter()
            .any(|row| row.id == "a")
    );
}

#[test]
fn scalar_filter_types_and_sort_permissions_are_checked_before_io() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    for filter in [
        Filter {
            field: "score".into(),
            operator: Operator::Eq,
            value: json!("2"),
        },
        Filter {
            field: "flag".into(),
            operator: Operator::Gt,
            value: json!(true),
        },
        Filter {
            field: "payload".into(),
            operator: Operator::Eq,
            value: json!(null),
        },
        Filter {
            field: "score".into(),
            operator: Operator::Gt,
            value: json!(null),
        },
    ] {
        assert!(
            r.open_view(
                "books",
                &ViewRequest {
                    filters: vec![filter],
                    ..all()
                }
            )
            .is_err()
        );
    }
    for req in [
        ViewRequest {
            sort: Some(Sort {
                field: "payload".into(),
                direction: Direction::Asc,
            }),
            ..all()
        },
        ViewRequest {
            limit: Some(0),
            ..all()
        },
        ViewRequest {
            columns: Some(vec![]),
            ..all()
        },
        ViewRequest {
            columns: Some(vec!["payload".into()]),
            ..all()
        },
        ViewRequest {
            columns: Some(vec!["title".into(), "title".into()]),
            ..all()
        },
    ] {
        assert!(r.open_view("books", &req).is_err())
    }
    assert_eq!(r.metrics().source_reads, 0);
    assert!(!f.data.exists());
}

#[test]
fn mixed_large_integer_and_fractional_comparison_is_exact() {
    let f = Fixture::new();
    let values = [
        json!(9007199254740993_i64),
        json!(9007199254740992.0),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(-9223372036854775808.0),
        json!(1.25),
        json!(-0.0),
    ];
    for (id, value) in ['a', 'b', 'c', 'd', 'e', 'f', 'g'].into_iter().zip(values) {
        f.edit(&format!("data/books/{id}.json"), |v| v["score"] = value)
    }
    assert_eq!(
        ids(&parity(&f, &all()).rows),
        vec!["d", "e", "g", "f", "b", "a", "c"]
    );
    let filter = Filter {
        field: "score".into(),
        operator: Operator::Gt,
        value: json!(9007199254740992_i64),
    };
    assert_eq!(
        ids(&parity(
            &f,
            &ViewRequest {
                filters: vec![filter],
                ..all()
            }
        )
        .rows),
        vec!["a", "c"]
    );
}

#[test]
fn numbers_outside_the_declared_domain_are_diagnosed_without_rounding() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |v| v["score"] = json!(u64::MAX));
    for mode in [CacheMode::Segments, CacheMode::Disabled] {
        let mut r = f.repo(mode);
        r.open_view("books", &all()).unwrap();
        r.validate_collection("books").unwrap();
        let view = r.open_view("books", &all()).unwrap();
        if mode == CacheMode::Segments {
            assert_eq!(view.states["a"].status, objekte_core::RecordStatus::Invalid);
            assert!(
                view.states["a"]
                    .issues
                    .iter()
                    .any(|i| i.message.contains("signed 64-bit"))
            );
        } else {
            assert_eq!(view.states["a"].status, objekte_core::RecordStatus::Pending);
            assert!(r.metrics().validations > 0);
        }
        assert_eq!(
            view.rows.iter().find(|r| r.id == "a").unwrap().values["score"],
            json!(u64::MAX)
        );
    }
}

#[test]
fn integral_json_floats_validate_as_integers_and_sort_correctly() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |v| v["count"] = json!(1.0));
    f.edit("data/books/b.json", |v| v["count"] = json!(2));
    let q = ViewRequest {
        sort: Some(Sort {
            field: "count".into(),
            direction: Direction::Desc,
        }),
        ..all()
    };
    assert_eq!(&ids(&parity(&f, &q).rows)[..2], &["b", "a"]);
    f.edit("data/books/a.json", |v| v["count"] = json!(1.5));
    assert_eq!(
        f.repo(CacheMode::Disabled)
            .open_view("books", &all())
            .unwrap()
            .states["a"]
            .status,
        objekte_core::RecordStatus::Pending
    );
}

#[test]
fn strings_are_binary_case_sensitive_and_not_markup_rendered() {
    let f = Fixture::new();
    for (id, title) in [
        ("a", "10"),
        ("b", "2"),
        ("c", "<b>HTML</b>"),
        ("d", "**Markdown**"),
        ("e", "a"),
        ("f", "Z"),
        ("g", "é"),
    ] {
        f.edit(&format!("data/books/{id}.json"), |v| {
            v["title"] = json!(title)
        })
    }
    let q = ViewRequest {
        sort: Some(Sort {
            field: "title".into(),
            direction: Direction::Asc,
        }),
        ..all()
    };
    let v = parity(&f, &q);
    assert_eq!(ids(&v.rows), vec!["d", "a", "b", "c", "f", "e", "g"]);
    assert_eq!(v.rows[0].label, "**Markdown**");
}

#[test]
fn empty_collections_and_offsets_beyond_end_are_valid() {
    let f = Fixture::new();
    for e in std::fs::read_dir(f.root.join("data/books")).unwrap() {
        std::fs::remove_file(e.unwrap().path()).unwrap()
    }
    let v = parity(&f, &all());
    assert!(v.rows.is_empty());
    assert_eq!(v.total, Some(0));
}

#[test]
fn punctuation_strings_remain_literal_values() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |v| v["status"] = json!("' OR 1=1; --"));
    let filter = Filter {
        field: "status".into(),
        operator: Operator::Eq,
        value: json!("' OR 1=1; --"),
    };
    assert_eq!(
        ids(&parity(
            &f,
            &ViewRequest {
                filters: vec![filter],
                ..all()
            }
        )
        .rows),
        vec!["a"]
    );
}

#[test]
fn ui_only_changes_rebuild_only_the_necessary_cache_parts() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    r.open_view("books", &all()).unwrap();
    f.edit(".editor/books/ui.json", |v| {
        v["label"] = json!("New label");
        v["pageSize"] = json!(2);
        v["list"]["columns"] = json!(["title"])
    });
    let v = r.open_view("books", &ViewRequest::default()).unwrap();
    assert_eq!(v.rows.len(), 2);
    assert_eq!(v.columns, vec!["title"]);
    assert_eq!(v.metrics.rows_updated, 0);
    assert_eq!(v.metrics.index_builds, 0);
    assert_eq!(v.metrics.parses, v.rows.len() as u64);
    f.edit(".editor/books/ui.json", |v| {
        v["list"]["sortable"]
            .as_array_mut()
            .unwrap()
            .push(json!("type"))
    });
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.index_builds, 0);
    assert_eq!(v.metrics.rows_updated, 0);
    assert_eq!(v.metrics.index_rows_written, 0);
    assert_eq!(v.metrics.validations, v.rows.len() as u64);
}

#[test]
fn generated_mutation_sequences_match_fresh_direct_execution() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Memory);
    let mut segments = f.repo(CacheMode::Segments);
    r.open_view("books", &all()).unwrap();
    segments.open_view("books", &all()).unwrap();
    let mut seed = 7u64;
    for step in 0..60 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let id = format!("random{}", seed % 13);
        let p = format!("data/books/{id}.json");
        if seed.is_multiple_of(4) {
            let _ = std::fs::remove_file(f.root.join(&p));
        } else {
            let score = if seed.is_multiple_of(5) {
                Value::Null
            } else {
                json!((seed % 2000) as f64 / 8.0 - 125.0)
            };
            f.write(&p,&json!({"title":format!("Title {}",seed%5),"score":score,"flag":seed.is_multiple_of(2),"status":if seed.is_multiple_of(3){"draft"}else{"reviewed"},"payload":null,"person":if seed.is_multiple_of(3){"p1"}else{"missing"}}));
        }
        r.refresh("books").unwrap();
        segments.refresh("books").unwrap();
        for field in ["score", "title", "flag", "@id"] {
            let q = ViewRequest {
                sort: Some(Sort {
                    field: field.into(),
                    direction: if step % 2 == 0 {
                        Direction::Asc
                    } else {
                        Direction::Desc
                    },
                }),
                filters: if step % 3 == 0 {
                    vec![Filter {
                        field: "status".into(),
                        operator: Operator::Eq,
                        value: json!("draft"),
                    }]
                } else {
                    vec![]
                },
                offset: step % 4,
                limit: Some(5),
                count: true,
                ..Default::default()
            };
            let memory = r.open_view("books", &q).unwrap();
            let packed = segments.open_view("books", &q).unwrap();
            let direct = f.repo(CacheMode::Disabled).open_view("books", &q).unwrap();
            assert_eq!(memory.rows, direct.rows, "step {step}, {field}");
            assert_eq!(memory.total, direct.total);
            assert_eq!(packed.rows, direct.rows, "segments step {step}, {field}");
            assert_eq!(packed.total, direct.total);
        }
    }
}

#[test]
fn out_of_range_integer_lexemes_are_diagnosed_while_opaque_numbers_are_preserved() {
    for number in [
        "-9223372036854775809",
        "18446744073709551616",
        "9999999999999999999999999999",
    ] {
        let f = Fixture::new();
        std::fs::write(
            f.root.join("data/books/a.json"),
            format!("{{\"payload\":null,\"score\":{number}}}"),
        )
        .unwrap();
        for mode in [CacheMode::Segments, CacheMode::Disabled] {
            let mut repo = f.repo(mode);
            repo.open_view("books", &all()).unwrap();
            repo.validate_collection("books").unwrap();
            let view = repo.open_view("books", &all()).unwrap();
            if mode == CacheMode::Segments {
                assert_eq!(view.states["a"].status, objekte_core::RecordStatus::Invalid);
                assert!(
                    view.states["a"]
                        .issues
                        .iter()
                        .any(|i| i.message.contains("signed 64-bit"))
                );
            } else {
                assert_eq!(view.states["a"].status, objekte_core::RecordStatus::Pending);
                assert!(repo.metrics().validations > 0);
            }
        }
        std::fs::write(
            f.root.join("data/books/a.json"),
            format!("{{\"payload\":{number},\"score\":1}}"),
        )
        .unwrap();
        let data = f
            .repo(CacheMode::Disabled)
            .get_record("books", "a")
            .unwrap()
            .unwrap()
            .data;
        assert_eq!(data["payload"].to_string(), number);
    }
}
