mod support;

use objekte_core::{
    CacheMode, Direction, Filter, Freshness, JsonQuery, JsonQueryBackend, Operator, Options,
    Repository, Sort, ViewRequest,
};
use serde_json::json;
use support::{Fixture, all};

#[test]
fn strict_reopen_rehashes_but_reuses_unchanged_segment_entries() {
    let f = Fixture::new();
    f.repo(CacheMode::Segments)
        .open_view("books", &all())
        .unwrap();
    let mut options = f.options(CacheMode::Segments);
    options.freshness = Freshness::HashOnOpen;
    let mut reopened = Repository::load_repository(&f.root, options).unwrap();
    let view = reopened.open_view("books", &all()).unwrap();
    assert_eq!(view.total, Some(7));
    assert_eq!(view.metrics.rows_reused, 7);
    assert_eq!(view.metrics.rows_updated, 0);
    assert!(view.metrics.hashes >= 7);
}

#[test]
fn equal_size_edit_with_restored_mtime_changes_revision() {
    use std::fs::{self, File, FileTimes};
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    let before = repo.record_revision("books", "a").unwrap().unwrap();
    let path = f.root.join("data/books/a.json");
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let original = fs::read_to_string(&path).unwrap();
    let edited = original.replace("Beta", "Zeta");
    assert_eq!(edited.len(), original.len());
    fs::write(&path, edited).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(mtime))
        .unwrap();
    repo.refresh("books").unwrap();
    let after = repo.record_revision("books", "a").unwrap().unwrap();
    assert!(after.revision > before.revision);
    assert_eq!(
        repo.get_record("books", "a").unwrap().unwrap().data["title"],
        "Zeta"
    );
}

#[test]
fn request_freshness_and_explicit_refresh_observe_external_edits() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    repo.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |record| record["score"] = json!(-5));
    assert!(matches!(
        repo.open_view("books", &all()),
        Err(objekte_core::Error::Unstable(_))
    ));
    assert_eq!(repo.refresh("books").unwrap().rows_updated, 1);
    assert_eq!(repo.open_view("books", &all()).unwrap().rows[0].id, "a");

    let mut options = Options::new(&f.data);
    options.cache = CacheMode::Segments;
    options.freshness = Freshness::HashOnRequest;
    let mut strict = Repository::load_repository(&f.root, options).unwrap();
    strict.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |record| record["score"] = json!(55));
    let view = strict.open_view("books", &all()).unwrap();
    assert_eq!(view.metrics.rows_updated, 1);
    assert_ne!(view.rows[0].id, "a");
}

#[test]
fn add_delete_and_rename_update_only_changed_entries() {
    use std::fs;
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    repo.open_view("books", &all()).unwrap();
    fs::rename(
        f.root.join("data/books/a.json"),
        f.root.join("data/books/z.json"),
    )
    .unwrap();
    fs::remove_file(f.root.join("data/books/c.json")).unwrap();
    f.write(
        "data/books/new.json",
        &json!({"title":"New","score":-10,"payload":null}),
    );
    let metrics = repo.refresh("books").unwrap();
    assert_eq!(metrics.rows_inserted, 2);
    assert_eq!(metrics.rows_deleted, 2);
    assert_eq!(metrics.rows_updated, 0);
    assert!(!repo.record_revision("books", "a").unwrap().unwrap().present);
    let cached = repo.open_view("books", &all()).unwrap();
    let direct = f
        .repo(CacheMode::Disabled)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(cached.rows, direct.rows);
}

#[test]
fn malformed_and_schema_invalid_edits_never_show_stale_values() {
    use std::fs;
    for bad in [
        "not json",
        "{\"payload\":null,\"score\":\"wrong\"}",
        "{\"score\":1}",
        "{\"payload\":null,\"extra\":true}",
    ] {
        let f = Fixture::new();
        let mut repo = f.repo(CacheMode::Segments);
        repo.open_view("books", &all()).unwrap();
        fs::write(f.root.join("data/books/a.json"), bad).unwrap();
        repo.refresh("books").unwrap();
        let view = repo.open_view("books", &all()).unwrap();
        let expected = if bad == "not json" {
            objekte_core::RecordStatus::IncorrectJson
        } else {
            objekte_core::RecordStatus::Invalid
        };
        assert_eq!(view.states["a"].status, expected);
        assert_eq!(view.total, Some(7));
        assert_ne!(
            view.rows
                .iter()
                .find(|row| row.id == "a")
                .unwrap()
                .values
                .get("title"),
            Some(&json!("Alpha"))
        );
        let reopened = f
            .repo(CacheMode::Segments)
            .open_view("books", &all())
            .unwrap();
        assert_eq!(reopened.states["a"].status, expected);
    }
}

#[test]
fn segment_query_tracks_edits_and_matches_direct() {
    let f = Fixture::new();
    let mut cached = f.repo(CacheMode::Segments);
    let mut direct = f.repo(CacheMode::Disabled);
    for filter in ["title ~ 'a'", "score >= 2", "score = 2", "flag = true"] {
        let request = JsonQuery {
            filter: filter.into(),
            collections: Some(vec!["books".into()]),
            count: true,
            backend: JsonQueryBackend::Segments,
            ..Default::default()
        };
        let expected = direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request.clone()
            })
            .unwrap();
        let actual = cached.query_json(&request).unwrap();
        assert_eq!(actual.total, expected.total, "{filter}");
        assert_eq!(
            actual.records.iter().map(|r| &r.id).collect::<Vec<_>>(),
            expected.records.iter().map(|r| &r.id).collect::<Vec<_>>(),
            "{filter}"
        );
    }
    let before = cached.cache_directory().unwrap();
    assert!(before.join("segments").exists());
    let request = JsonQuery {
        filter: "score >= 2".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        backend: JsonQueryBackend::Segments,
        ..Default::default()
    };
    let initial_revision = cached.query_json(&request).unwrap().dependencies.revisions["books"];
    f.edit("data/books/a.json", |value| value["score"] = 9.into());
    cached.refresh("books").unwrap();
    let changed = cached.query_json(&request).unwrap();
    assert!(changed.dependencies.revisions["books"] > initial_revision);
    assert_eq!(
        changed.total,
        direct
            .query_json(&JsonQuery {
                backend: JsonQueryBackend::Direct,
                ..request
            })
            .unwrap()
            .total
    );
}

#[test]
fn segment_views_match_direct_with_numeric_sort_and_filter() {
    let f = Fixture::new();
    let mut cached = f.repo(CacheMode::Segments);
    let mut direct = f.repo(CacheMode::Disabled);
    let requests = [
        all(),
        ViewRequest {
            sort: Some(Sort {
                field: "score".into(),
                direction: Direction::Asc,
            }),
            count: true,
            ..all()
        },
        ViewRequest {
            sort: Some(Sort {
                field: "score".into(),
                direction: Direction::Desc,
            }),
            filters: vec![Filter {
                field: "score".into(),
                operator: Operator::Ge,
                value: json!(2),
            }],
            count: true,
            ..all()
        },
    ];
    for request in requests {
        let expected = direct.open_view("books", &request).unwrap();
        let actual = cached.open_view("books", &request).unwrap();
        assert_eq!(actual.rows, expected.rows);
        assert!(
            actual
                .states
                .values()
                .all(|state| state.status == objekte_core::RecordStatus::Valid)
        );
        assert_eq!(actual.total, expected.total);
        if request
            .sort
            .as_ref()
            .is_some_and(|sort| sort.field == "score")
        {
            assert_eq!(actual.metrics.source_reads as usize, actual.rows.len());
        }
    }
}

#[test]
fn stale_text_is_compacted_and_scalar_cardinality_is_checked_globally() {
    let f = Fixture::new();
    let mut cached = f.repo(CacheMode::Segments);
    let request = JsonQuery {
        filter: "title ~ 'Alpha'".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        backend: JsonQueryBackend::Segments,
        ..Default::default()
    };
    cached.query_json(&request).unwrap();
    for n in 0..8 {
        f.edit("data/books/a.json", |value| {
            value["title"] = json!(format!("Changed {n}"))
        });
        cached.refresh("books").unwrap();
    }
    let names = std::fs::read_dir(cached.cache_directory().unwrap().join("segments"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    let collection_dir = &names[0];
    let text_files = std::fs::read_dir(collection_dir)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("text-")
        })
        .count();
    assert!(text_files <= 7);
    let active_text = std::fs::read_dir(collection_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("text-")
        })
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<String>();
    assert!(active_text.contains("Changed 7"));
    assert!(!active_text.contains("Changed 0"));
    let stale = cached.query_json(&request).unwrap();
    assert!(!stale.records.iter().any(|record| record.id == "a"));

    f.edit("data/books/a.json", |value| {
        value["title"] = json!(["No", "Match"])
    });
    cached.refresh("books").unwrap();
    let request = JsonQuery {
        filter: "title ~ 'Alpha'".into(),
        ..request
    };
    assert!(cached.query_json(&request).is_err());
}

#[test]
fn enum_fields_get_ordered_cache_keys() {
    let f = Fixture::new();
    f.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["status"]["enum"] = json!(["draft", "reviewed", null]);
    });
    let mut cached = f.repo(CacheMode::Segments);
    assert!(
        cached.model().collections["books"].fields["status"]
            .enum_values
            .is_some()
    );
    let request = ViewRequest {
        sort: Some(Sort {
            field: "status".into(),
            direction: Direction::Asc,
        }),
        ..all()
    };
    let actual = cached.open_view("books", &request).unwrap();
    let expected = f
        .repo(CacheMode::Disabled)
        .open_view("books", &request)
        .unwrap();
    assert_eq!(actual.rows, expected.rows);
    assert!(
        actual
            .query_plan
            .iter()
            .any(|line| line.contains("ordered segment index"))
    );
}

#[test]
fn configuration_and_record_revisions_survive_reopen() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    let first = repo.refresh_configuration().unwrap();
    assert!(first.persistent);
    assert!(!first.inputs.is_empty());
    assert!(repo.refresh_configuration().unwrap().inputs.is_empty());
    let mut reopened = f.repo(CacheMode::Segments);
    assert!(reopened.refresh_configuration().unwrap().inputs.is_empty());
    let before = reopened.open_view("books", &all()).unwrap();
    assert_eq!(before.total, Some(7));
    let revision = reopened
        .cache_status()
        .unwrap()
        .collections
        .iter()
        .find(|c| c.collection == "books")
        .unwrap()
        .revision;
    f.edit("data/books/a.json", |value| value["score"] = json!(42));
    reopened.refresh("books").unwrap();
    let later = f.repo(CacheMode::Segments).cache_status().unwrap();
    let changed = later
        .collections
        .iter()
        .find(|c| c.collection == "books")
        .unwrap();
    assert!(changed.revision > revision);
}

#[test]
fn deleted_ids_keep_revision_and_recreation_advances_it() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    let first = repo.record_revision("books", "a").unwrap().unwrap();
    assert!(first.present);
    assert_eq!(first.revision, 1);
    std::fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    repo.refresh("books").unwrap();
    let deleted = f
        .repo(CacheMode::Segments)
        .record_revision("books", "a")
        .unwrap()
        .unwrap();
    assert!(!deleted.present);
    assert_eq!(deleted.revision, 2);
    assert!(deleted.changed_at_nanos >= first.changed_at_nanos);
    f.write(
        "data/books/a.json",
        &json!({"title":"Recreated","score":3,"payload":{}}),
    );
    repo.refresh("books").unwrap();
    let revived = f
        .repo(CacheMode::Segments)
        .record_revision("books", "a")
        .unwrap()
        .unwrap();
    assert!(revived.present);
    assert_eq!(revived.revision, 3);
    assert!(revived.changed_at_nanos >= deleted.changed_at_nanos);
}

#[test]
fn validation_runs_from_sources_and_is_not_stored_in_cache() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |value| value["score"] = json!("wrong"));
    let mut repo = f.repo(CacheMode::Segments);
    let metrics = repo.validate_collection("books").unwrap();
    assert_eq!(metrics.validated, 7);
    let record = repo.get_record("books", "a").unwrap().unwrap();
    assert_eq!(record.state.status, objekte_core::RecordStatus::Invalid);
    let status = repo.cache_status().unwrap();
    assert!(
        status
            .collections
            .iter()
            .all(|collection| collection.validation.is_empty())
    );
}

#[test]
fn damaged_snapshot_is_rebuilt_from_source() {
    let f = Fixture::new();
    let request = JsonQuery {
        filter: "score >= 2".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        backend: JsonQueryBackend::Segments,
        ..Default::default()
    };
    let mut first = f.repo(CacheMode::Segments);
    let expected = first.query_json(&request).unwrap().total;
    let directory = std::fs::read_dir(first.cache_directory().unwrap().join("segments"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    std::fs::write(directory.join("CURRENT.json"), b"broken").unwrap();
    let mut reopened = f.repo(CacheMode::Segments);
    assert_eq!(reopened.query_json(&request).unwrap().total, expected);
}

#[test]
fn incompatible_cache_version_is_rebuilt() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    repo.open_view("books", &all()).unwrap();
    let directory = std::fs::read_dir(repo.cache_directory().unwrap().join("segments"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    let current = directory.join("CURRENT.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&current).unwrap()).unwrap();
    manifest["version"] = json!(5);
    std::fs::write(&current, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let mut reopened = f.repo(CacheMode::Segments);
    assert_eq!(reopened.open_view("books", &all()).unwrap().total, Some(7));
    let rebuilt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(current).unwrap()).unwrap();
    assert_eq!(rebuilt["version"], 6);
}

#[test]
fn concurrent_builders_publish_one_complete_snapshot() {
    let f = Fixture::new();
    let root = f.root.clone();
    let data = f.data.clone();
    let handles = (0..2)
        .map(|_| {
            let root = root.clone();
            let data = data.clone();
            std::thread::spawn(move || {
                let mut options = objekte_core::Options::new(data);
                options.cache = CacheMode::Segments;
                let mut repo = objekte_core::Repository::load_repository(root, options).unwrap();
                let request = JsonQuery {
                    filter: "score >= 2".into(),
                    collections: Some(vec!["books".into()]),
                    count: true,
                    backend: JsonQueryBackend::Segments,
                    ..Default::default()
                };
                repo.query_json(&request).unwrap().total
            })
        })
        .collect::<Vec<_>>();
    let totals = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(totals[0], totals[1]);
}

#[test]
fn numeric_fields_outside_view_configuration_are_indexed() {
    let f = Fixture::new();
    f.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["private_metric"] = json!({"type":"number"});
        schema["properties"]["private_flag"] = json!({"type":"boolean"});
        schema["properties"]["private_status"] = json!({"type":"string","enum":["draft","final"]});
    });
    f.edit("data/books/a.json", |record| {
        record["private_metric"] = json!(6);
        record["private_flag"] = json!(true);
        record["private_status"] = json!("final");
    });
    f.edit("data/books/b.json", |record| {
        record["private_metric"] = json!(2);
        record["private_flag"] = json!(false);
        record["private_status"] = json!("draft");
    });
    let mut repo = f.repo(CacheMode::Segments);
    let indexes = repo.model().collections["books"]
        .indexes
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    for field in ["private_metric", "private_flag", "private_status"] {
        assert!(indexes.contains(&field));
    }
    let request = JsonQuery {
        filter: "private_metric > 5".into(),
        collections: Some(vec!["books".into()]),
        count: true,
        backend: JsonQueryBackend::Segments,
        ..Default::default()
    };
    repo.query_json(&request).unwrap();
    repo.reset_metrics();
    let result = repo.query_json(&request).unwrap();
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].id, "a");
    assert_eq!(result.metrics.source_reads, 2);
    assert_eq!(result.metrics.grep_scans, 0);
    for (filter, id) in [
        ("private_flag = true", "a"),
        ("private_status = 'draft'", "b"),
    ] {
        let result = repo
            .query_json(&JsonQuery {
                filter: filter.into(),
                collections: Some(vec!["books".into()]),
                count: true,
                backend: JsonQueryBackend::Segments,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(result.total, Some(1));
        assert_eq!(result.records[0].id, id);
    }
}
