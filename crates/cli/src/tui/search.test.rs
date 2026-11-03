use super::*;
use crate::tui::{
    app::{Overlay, View},
    input::Input,
    navigation::DetailSection,
};
use objekte_core::{JsonQuery, Options, Repository, TextQuery};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    fs::{self, File, FileTimes},
    sync::mpsc::TrySendError,
    time::{Duration, Instant, UNIX_EPOCH},
};

#[test]
fn search_channel_fits_a_page_and_completion_without_ui_draining() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let record = repo.get_record("docs", "a").unwrap().unwrap();
    let (sender, _receiver) = search_channel();
    for _ in 0..PAGE_SIZE {
        sender
            .try_send(SearchUpdate::Hit(Box::new(Hit::Filter(record.clone()))))
            .unwrap_or_else(|error| match error {
                TrySendError::Full(_) => panic!("search page blocked on the UI"),
                TrySendError::Disconnected(_) => panic!("search channel disconnected"),
            });
    }
    assert!(
        sender
            .try_send(SearchUpdate::Finished {
                repository: Box::new(repo),
                total: Ok(Some(PAGE_SIZE)),
                truncated: false,
                elapsed_ms: Some(1.0),
            })
            .is_ok()
    );
}

#[test]
fn streamed_hits_reach_the_ui_before_the_final_count() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon A"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"note":"Moon B"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let first = repo
        .search_text(&TextQuery {
            pattern: "Moon".into(),
            ..Default::default()
        })
        .unwrap()
        .hits
        .remove(0);
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon");
    app.search();
    app.pending_search.take();

    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: app.generation,
            kind: JobKind::Search,
        }),
    };
    sender
        .send(SearchUpdate::Hit(Box::new(Hit::Text(first))))
        .unwrap();
    worker.drive(&mut app);
    assert_eq!(app.hits.len(), 1);
    assert!(app.searching);
    assert_eq!(app.total, None);
    assert_eq!(app.elapsed_ms, None);
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Ok(Some(2)),
            truncated: false,
            elapsed_ms: Some(12.5),
        })
        .unwrap();
    worker.drive(&mut app);
    assert!(!app.searching);
    assert_eq!(app.total, Some(2));
    assert_eq!(app.elapsed_ms, Some(12.5));
    assert_eq!(app.hits[0].record().id, "a");
}

#[test]
fn background_tui_searches_reuse_verified_text_and_filter_pages() {
    let temp = tempfile::tempdir().unwrap();
    let schema = temp.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        r#"{"type":"object","properties":{"note":{"type":"string"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon A"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"note":"Moon B"}"#).unwrap();

    let mut options = Options::new();
    options.stream_query_cache = true;
    let mut repo = Repository::load_repository(temp.path(), options).unwrap();
    let mut app = App::new(&mut repo);
    app.async_search = true;
    let mut worker = SearchWorker::default();
    app.search = Input::with_text("Moon");
    app.search();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(2));
    assert!(app.repo.query_cache_bytes() > 0);

    app.repo.reset_metrics();
    app.search();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.hits.len(), 2);
    assert!(app.repo.metrics().ordered_result_hits > 0);

    app.search.clear();
    app.filter = Input::with_text("note ~ 'Moon'");
    app.search();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(2));

    app.repo.reset_metrics();
    app.search();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.hits.len(), 2);
    assert!(app.repo.metrics().ordered_result_hits > 0);
}

#[test]
fn sorted_page_updates_keep_selection_by_record_identity() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for (id, title) in [("a", "Zulu"), ("b", "Alpha"), ("c", "Beta")] {
        fs::write(
            collection.join(format!("{id}.json")),
            format!(r#"{{"title":"{title}","note":"Moon"}}"#),
        )
        .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let records = ["a", "b", "c"]
        .into_iter()
        .map(|id| repo.get_record("docs", id).unwrap().unwrap())
        .collect::<Vec<_>>();
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon SORT BY title");
    app.search();
    app.pending_search.take();
    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: app.generation,
            kind: JobKind::Search,
        }),
    };
    sender
        .send(SearchUpdate::Page(vec![
            Hit::Filter(records[0].clone()),
            Hit::Filter(records[2].clone()),
        ]))
        .unwrap();
    worker.drive(&mut app);
    app.selected = 0;
    sender
        .send(SearchUpdate::Page(vec![
            Hit::Filter(records[1].clone()),
            Hit::Filter(records[2].clone()),
            Hit::Filter(records[0].clone()),
        ]))
        .unwrap();
    worker.drive(&mut app);
    assert_eq!(app.hits[app.selected].record().id, "a");
    assert_eq!(app.selected, 2);
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Ok(Some(3)),
            truncated: false,
            elapsed_ms: Some(1.0),
        })
        .unwrap();
    worker.drive(&mut app);
    assert!(!app.searching);
}

#[test]
fn real_query_worker_streams_ordered_hits_and_restores_repository() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for id in ["b", "a", "c"] {
        let path = collection.join(format!("{id}.json"));
        fs::write(&path, format!(r#"{{"note":"Moon {id}"}}"#)).unwrap();
        let seconds = match id {
            "a" => 300,
            "b" => 200,
            _ => 100,
        };
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
            .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon");
    app.search();
    let mut worker = SearchWorker::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(3));
    assert!(app.elapsed_ms.is_some());
    assert_eq!(
        app.hits
            .iter()
            .map(|hit| hit.record().id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        app.repo
            .search_text(&TextQuery {
                pattern: "Moon".into(),
                count: true,
                ..Default::default()
            })
            .unwrap()
            .total,
        Some(3)
    );
}

#[test]
fn inference_worker_keeps_ui_responsive_and_returns_its_warm_repository() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for id in 0..24 {
        fs::write(
            collection.join(format!("{id}.json")),
            format!(
                r#"{{"active":{},"kind":"{}"}}"#,
                id % 2 == 0,
                if id % 2 == 0 { "A" } else { "B" }
            ),
        )
        .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.pending_inference = Some(crate::tui::app::InferRequest {
        collection: Some("docs".into()),
        auto_index: true,
    });
    let mut worker = SearchWorker::default();
    worker.drive(&mut app);
    assert!(app.index_status.is_some());
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.index_status.is_some() && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(app.index_status.is_none());
    assert!(matches!(app.overlay, Some(Overlay::InferReady)));
    let result = app
        .repo
        .query_json(&JsonQuery {
            filter: "active = true".into(),
            collections: Some(vec!["docs".into()]),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(12));
    assert_eq!(result.metrics.grep_scans, 0);
}

#[test]
fn completed_advanced_scan_offers_a_separate_schema_write() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for id in 0..24 {
        fs::write(
            collection.join(format!("{id}.json")),
            format!(r#"{{"active":{},"detail":{{"rank":{id}}}}}"#, id % 2 == 0),
        )
        .unwrap();
    }
    let schema_path = temp.path().join(".editor/collections/docs/schema.json");
    fs::create_dir_all(schema_path.parent().unwrap()).unwrap();
    fs::write(&schema_path, r#"{"type":"object","properties":{"active":{"type":"string","index":false,"exclude_fts":true}}}"#).unwrap();
    let original = fs::read(&schema_path).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.pending_inference = Some(crate::tui::app::InferRequest {
        collection: Some("docs".into()),
        auto_index: true,
    });
    let mut worker = SearchWorker::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while (app.pending_inference.is_some() || worker.running.is_some()) && Instant::now() < deadline
    {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(app.overlay, Some(Overlay::InferReady)));
    assert_eq!(fs::read(&schema_path).unwrap(), original);
    crate::tui::keys::key(
        &mut app,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
    );
    assert!(app.pending_schema_write);
    while (app.pending_schema_write || worker.running.is_some()) && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        app.inference_saved,
        "{:?}",
        app.notice.as_ref().map(|notice| &notice.text)
    );
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    assert_eq!(schema["properties"]["active"]["type"], "boolean");
    assert_eq!(schema["properties"]["active"]["index"], true);
    assert_eq!(schema["properties"]["active"]["exclude_fts"], true);
    assert!(
        schema["x-index-paths"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("detail.rank"))
    );
    let mut reopened = Repository::open(temp.path()).unwrap();
    let result = reopened
        .query_json(&JsonQuery {
            filter: "detail.rank >= 20".into(),
            count: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(4));
    assert_eq!(result.metrics.grep_scans, 0);
}

#[test]
fn filter_query_uses_the_same_hit_event_path() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"note":"Sun"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.filter = Input::with_text("note ~ 'Moon'");
    app.search();
    let mut worker = SearchWorker::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(1));
    assert!(matches!(app.hits.as_slice(), [Hit::Filter(_)]));
    assert_eq!(app.hits[0].record().id, "a");
}

#[test]
fn late_stream_error_discards_provisional_rows() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let first = repo
        .search_text(&TextQuery {
            pattern: "Moon".into(),
            ..Default::default()
        })
        .unwrap()
        .hits
        .remove(0);
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon");
    app.search();
    app.pending_search.take();
    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: app.generation,
            kind: JobKind::Search,
        }),
    };
    sender
        .send(SearchUpdate::Hit(Box::new(Hit::Text(first))))
        .unwrap();
    worker.drive(&mut app);
    assert_eq!(app.hits.len(), 1);
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Err("source changed during search".into()),
            truncated: false,
            elapsed_ms: None,
        })
        .unwrap();
    worker.drive(&mut app);
    assert!(app.hits.is_empty());
    assert!(!app.searching);
    assert!(app.last_request.is_none());
    assert!(app.view == View::Results);
    let notice = app.notice.as_ref().unwrap();
    assert!(notice.error && notice.text.contains("source changed"));
}

#[test]
fn changed_query_discards_old_events_then_runs_latest_request() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"note":"Moon"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"note":"Sun"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let old_hit = repo
        .search_text(&TextQuery {
            pattern: "Moon".into(),
            ..Default::default()
        })
        .unwrap()
        .hits
        .remove(0);
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon");
    app.search();
    app.pending_search.take();
    let old_generation = app.generation;
    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::clone(&cancel),
            generation: old_generation,
            kind: JobKind::Search,
        }),
    };
    app.search = Input::with_text("Sun");
    app.search();
    sender
        .send(SearchUpdate::Hit(Box::new(Hit::Text(old_hit))))
        .unwrap();
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Ok(Some(1)),
            truncated: false,
            elapsed_ms: Some(99.0),
        })
        .unwrap();
    worker.drive(&mut app);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.hits.is_empty());
    assert!(app.searching);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "b");
}

#[test]
fn detail_navigation_reopens_the_correct_hit_after_streamed_page_changes() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for number in 0..101 {
        let path = collection.join(format!("{number:03}.json"));
        fs::write(&path, r#"{"note":"Moon"}"#).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000 - number)),
            )
            .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.async_search = true;
    app.search = Input::with_text("Moon");
    let mut worker = SearchWorker::default();
    app.search();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.total, Some(101));
    assert_eq!(app.hits.len(), 100);
    app.selected = 99;
    app.open_detail();
    // Stepping past the last loaded row loads the next rows first.
    app.detail_step(true);
    assert!(app.searching);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert!(app.view == View::Detail);
    assert_eq!(app.hits.len(), 101);
    assert_eq!(app.selected, 100);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "100");
    app.detail_step(false);
    assert_eq!(app.current_detail_hit().unwrap().record().id, "099");
}


#[test]
fn backlink_event_mounts_a_selectable_row_before_lookup_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let books = temp.path().join("data/books");
    let people = temp.path().join("data/people");
    fs::create_dir_all(books.join(".editor")).unwrap();
    fs::create_dir_all(&people).unwrap();
    fs::write(
        books.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"person":{"type":"string","x-relation":{"collection":"people"}}}}"#,
    )
    .unwrap();
    fs::write(books.join("b.json"), r#"{"person":"p"}"#).unwrap();
    fs::write(people.join("p.json"), r#"{"name":"Alice"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Alice");
    app.search();
    app.open_detail();
    assert!(matches!(app.links.status, BacklinkStatus::Loading));
    let source = app.repo.get_record("books", "b").unwrap().unwrap();
    app.links.pending.take();
    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: app.links.generation,
            kind: JobKind::Backlinks,
        }),
    };
    sender
        .send(SearchUpdate::Backlink(Box::new(source)))
        .unwrap();
    worker.drive(&mut app);
    assert_eq!(app.links.incoming.len(), 1);
    assert!(matches!(app.links.status, BacklinkStatus::Loading));
    app.links.section = DetailSection::Backlinks;
    app.open_selected_relation();
    assert_eq!(app.current_detail_hit().unwrap().record().id, "b");
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Ok(None),
            truncated: false,
            elapsed_ms: None,
        })
        .unwrap();
    worker.drive(&mut app);
    assert_eq!(app.links.outgoing.len(), 1);
    assert!(matches!(app.links.status, BacklinkStatus::Complete));
}

#[test]
fn new_query_discards_stale_backlinks_and_reuses_the_warm_repository() {
    let temp = tempfile::tempdir().unwrap();
    let books = temp.path().join("data/books");
    let people = temp.path().join("data/people");
    fs::create_dir_all(books.join(".editor")).unwrap();
    fs::create_dir_all(&people).unwrap();
    fs::write(
        books.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"person":{"type":"string","x-relation":{"collection":"people"}}}}"#,
    )
    .unwrap();
    fs::write(books.join("b.json"), r#"{"title":"Moon","person":"p"}"#).unwrap();
    fs::write(people.join("p.json"), r#"{"name":"Alice"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Alice");
    app.search();
    app.open_detail();
    let source = app.repo.get_record("books", "b").unwrap().unwrap();
    app.links.pending.take();
    let old_generation = app.links.generation;
    let placeholder = app.repo.fork_unindexed();
    let repository = std::mem::replace(app.repo, placeholder);
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut worker = SearchWorker {
        running: Some(Running {
            receiver,
            cancel: Arc::clone(&cancel),
            generation: old_generation,
            kind: JobKind::Backlinks,
        }),
    };
    app.async_search = true;
    app.search = Input::with_text("Moon");
    app.search();
    sender
        .send(SearchUpdate::Backlink(Box::new(source)))
        .unwrap();
    sender
        .send(SearchUpdate::Finished {
            repository: Box::new(repository),
            total: Ok(None),
            truncated: false,
            elapsed_ms: None,
        })
        .unwrap();
    worker.drive(&mut app);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.links.incoming.is_empty());
    assert!(app.searching);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.searching && Instant::now() < deadline {
        worker.drive(&mut app);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.searching);
    assert_eq!(app.total, Some(1));
    assert_eq!(app.hits[0].record().id, "b");
}
