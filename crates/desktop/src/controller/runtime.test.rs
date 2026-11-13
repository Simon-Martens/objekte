use super::*;

fn large_delta_fixture(records: usize) -> (tempfile::TempDir, OpenedSession, IndexView, IndexView) {
    use objekte_search_core::lifecycle::Index;
    use objekte_search_core::lifecycle_caller::IndexedRepository;
    use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
    use objekte_search_core::source::SourceFormat;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let repository =
        Repository::open_with_options(root.path(), vec![definition], RepositoryOptions::default())
            .unwrap();
    let index = Index::from_repository(repository).unwrap();
    let previous = index.view().unwrap();
    let path = root.path().join("data/large.jsonl");
    let mut contents = String::new();
    for number in 0..records {
        contents.push_str(&format!("{{\"name\":\"n{number}\"}}\n"));
    }
    std::fs::write(&path, contents).unwrap();
    index.add_file(&path).unwrap();
    let current = index.view().unwrap();
    let mut session = crate::backend::test_support::fixture().session;
    session.repository = Arc::new(IndexedRepository::new(index).unwrap());
    session.collections = vec!["data".into()];
    session.single_collection = true;
    (root, session, previous, current)
}

#[test]
fn completed_inference_refreshes_fields_without_rows_or_edits() {
    use crate::ui::filter_builder::{Comparison, ConditionDraft, ValueType, serialize_conditions};
    use objekte_search_core::inference::{InferenceConfig, InferenceMode};
    use objekte_search_core::lifecycle::Index;
    use objekte_search_core::lifecycle_caller::IndexedRepository;
    use objekte_search_core::model::{FieldPath, LogicalType, PathRoot};
    use objekte_search_core::reference::FieldDefinition;
    use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
    use objekte_search_core::source::SourceFormat;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::create_dir(root.path().join("other")).unwrap();
    for number in 0..2_050 {
        let body = match number {
            2_048 => r#"{"name":"late","extra":{"rare.field":42,"mixed":true}}"#.to_owned(),
            2_049 => r#"{"name":"last","extra":{"mixed":7}}"#.to_owned(),
            _ => format!(r#"{{"name":"n{number}"}}"#),
        };
        std::fs::write(root.path().join(format!("data/f{number:04}.json")), body).unwrap();
    }
    std::fs::write(root.path().join("other/a.json"), r#"{"only_other":"yes"}"#).unwrap();
    let mut data = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    data.schema
        .register_field(FieldDefinition {
            path: vec!["declared".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let other = CollectionDefinition::folder("other", "other", SourceFormat::Json);
    let index = Index::start_incremental_build(
        root.path(),
        vec![data, other],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let path = FieldPath {
        root: PathRoot::Data,
        segments: vec!["extra".into(), "rare.field".into()],
    }
    .display();
    let choices = field_choices(&view, Some("data"), &[]);
    assert_eq!(
        choices
            .iter()
            .find(|choice| choice.path == path)
            .unwrap()
            .value_type,
        Some(ValueType::Number)
    );
    assert_eq!(
        choices
            .iter()
            .find(|choice| choice.path == "extra.mixed")
            .unwrap()
            .value_type,
        None
    );
    assert_eq!(
        choices
            .iter()
            .find(|choice| choice.path == "declared")
            .unwrap()
            .value_type,
        Some(ValueType::String)
    );
    assert!(choices.iter().any(|choice| choice.path == "@record.id"));
    assert!(choices.iter().all(|choice| choice.path != "only_other"));
    assert!(
        field_choices(&view, Some("other"), &[])
            .iter()
            .any(|choice| choice.path == "only_other")
    );

    let mut session = crate::backend::test_support::fixture().session;
    session.repository = Arc::new(IndexedRepository::new(index.clone()).unwrap());
    session.collections = vec!["data".into(), "other".into()];
    session.single_collection = false;
    let mut controller = DesktopController::new().0;
    controller.session = Some(session.clone());
    controller.open_generation = 3;
    controller.current_draft.collection = Some("data".into());
    controller.current_draft.structured = "name = 'no such record'".into();
    controller.current_draft.filter_enabled = true;
    controller.initial_task = Some(InitialBuildTask::from_ready(index).unwrap());
    let mut updates = Vec::new();
    controller.poll_index_tasks(&mut updates);
    let suggested = updates
        .iter()
        .find_map(|update| match update {
            UiUpdate::FieldChoices(fields) => Some(fields),
            _ => None,
        })
        .unwrap();
    assert_eq!(suggested, &choices);
    assert!(controller.initial_task.is_none());
    let mut repeat = Vec::new();
    controller.push_field_choices(&mut repeat);
    assert!(repeat.is_empty());
    controller.current_draft.collection = Some("other".into());
    controller.push_field_choices(&mut repeat);
    let other_choices = repeat
        .iter()
        .find_map(|update| match update {
            UiUpdate::FieldChoices(fields) => Some(fields),
            _ => None,
        })
        .unwrap();
    assert!(
        other_choices
            .iter()
            .any(|choice| choice.path == "only_other")
    );
    assert!(other_choices.iter().all(|choice| choice.path != path));
    repeat.clear();
    controller.open_generation += 1;
    controller.push_field_choices(&mut repeat);
    assert!(matches!(repeat.as_slice(), [UiUpdate::FieldChoices(_)]));

    let inserted = serialize_conditions(&[ConditionDraft {
        field: path.clone(),
        predicate: None,
        comparison: Comparison::Equal,
        value: Some("42".into()),
        value_type: Some(ValueType::Number),
    }])
    .unwrap()
    .unwrap();
    let manual = format!("{path} = 42");
    let selected = backend::QuerySpec {
        filter: inserted,
        scope: Some("data".into()),
        ..Default::default()
    };
    let expected = backend::QuerySpec {
        filter: manual,
        scope: Some("data".into()),
        ..Default::default()
    };
    let selected_page = backend::query_page(
        &session,
        &selected,
        0,
        backend::MAX_PAGE_ROWS,
        &CancellationToken::new(),
    )
    .unwrap();
    let manual_page = backend::query_page(
        &session,
        &expected,
        0,
        backend::MAX_PAGE_ROWS,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(selected_page.hits.len(), 1);
    assert_eq!(
        selected_page.hits[0].record_id,
        manual_page.hits[0].record_id
    );
}

#[test]
fn desktop_delta_bounds_bytes_and_evidence_before_materialization() {
    let (_root, session, previous, current) = large_delta_fixture(2_048);
    let spec = backend::QuerySpec::default();
    let token = CancellationToken::new();
    let query = backend::build_query(&spec).unwrap();
    assert_eq!(
        query.options.return_mode,
        objekte_search_core::workload::ReturnMode::Bytes
    );
    assert!(query.options.evidence);
    let all = current
        .search_added_since_with_cancellation(&previous, &query, &token)
        .unwrap()
        .unwrap();
    assert_eq!(all.result.hits.len(), 2_048);
    assert_eq!(all.result.counters.selected_payload_reads, 2_048);

    let (kind, page) =
        incremental_delta_page(&session, &previous, &current, &spec, 0, &token).unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(0)));
    assert_eq!(page.hits.len(), backend::MAX_PAGE_ROWS);
    assert_eq!(
        page.counters.returned_records,
        backend::MAX_PAGE_ROWS as u64
    );
    assert_eq!(
        page.counters.selected_payload_reads,
        backend::MAX_PAGE_ROWS as u64
    );
    assert!(page.counters.selected_payload_bytes > 0);
    assert!(page.counters.peak_retained_payload_bytes <= page.counters.selected_payload_bytes);
    assert!(page.hits.iter().all(|hit| hit.payload.is_some()));
    assert!(page.provisional && page.count.is_none() && page.next_page_start.is_none());

    let (kind, one) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &spec,
        backend::MAX_PAGE_ROWS - 1,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(_)));
    assert_eq!(one.hits.len(), 1);
    assert_eq!(one.counters.selected_payload_reads, 1);
    let (kind, full) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &spec,
        backend::MAX_PAGE_ROWS,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(_)));
    assert!(full.hits.is_empty());
    assert_eq!(full.counters.selected_payload_reads, 0);
    assert_eq!(full.counters.selected_payload_bytes, 0);
    assert!(full.provisional);

    let limited = backend::QuerySpec {
        limit: Some(3),
        ..spec.clone()
    };
    let (_, limited_one) =
        incremental_delta_page(&session, &previous, &current, &limited, 2, &token).unwrap();
    assert_eq!(limited_one.hits.len(), 1);
    assert_eq!(limited_one.counters.selected_payload_reads, 1);
    let (_, limited_full) =
        incremental_delta_page(&session, &previous, &current, &limited, 3, &token).unwrap();
    assert!(limited_full.hits.is_empty());
    assert_eq!(limited_full.counters.selected_payload_reads, 0);
}

#[test]
fn larger_publication_does_not_increase_selected_payload_work() {
    let (_root, session, previous, current) = large_delta_fixture(8_192);
    let spec = backend::QuerySpec::default();
    let (kind, page) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &spec,
        0,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(0)));
    assert_eq!(page.hits.len(), backend::MAX_PAGE_ROWS);
    assert_eq!(
        page.counters.selected_payload_reads,
        backend::MAX_PAGE_ROWS as u64
    );
    assert_eq!(
        page.counters.returned_records,
        backend::MAX_PAGE_ROWS as u64
    );
    assert!(page.counters.selected_payload_bytes > 0);
    assert!(page.counters.peak_retained_payload_bytes <= page.counters.selected_payload_bytes);
}

#[test]
fn full_delta_skip_preserves_replacement_and_new_query_access() {
    let (_root, session, previous, current) = large_delta_fixture(512);
    let token = CancellationToken::new();
    let default_spec = backend::QuerySpec::default();
    let (kind, full) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &default_spec,
        backend::MAX_PAGE_ROWS,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(_)));
    assert!(full.hits.is_empty());

    let changed = backend::QuerySpec {
        filter: "name = 'n511'".into(),
        ..Default::default()
    };
    let (kind, found) =
        incremental_delta_page(&session, &previous, &current, &changed, 0, &token).unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(0)));
    assert_eq!(found.hits.len(), 1);

    let counted = backend::QuerySpec {
        count: true,
        ..Default::default()
    };
    let (kind, replacement) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &counted,
        backend::MAX_PAGE_ROWS,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::First));
    assert_eq!(replacement.hits.len(), backend::MAX_PAGE_ROWS);
    assert!(!replacement.provisional);
    let offset = backend::QuerySpec {
        offset: 1,
        ..Default::default()
    };
    let (kind, replacement) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &offset,
        backend::MAX_PAGE_ROWS,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::First));
    assert_eq!(replacement.hits.len(), backend::MAX_PAGE_ROWS);
    let sorted = backend::QuerySpec {
        filter: "name:? = true SORT BY name DESC".into(),
        ..Default::default()
    };
    let (kind, replacement) = incremental_delta_page(
        &session,
        &previous,
        &current,
        &sorted,
        backend::MAX_PAGE_ROWS,
        &token,
    )
    .unwrap();
    assert!(matches!(kind, GridRequestKind::First));
    assert_eq!(replacement.hits.len(), backend::MAX_PAGE_ROWS);

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        incremental_delta_page(&session, &previous, &current, &default_spec, 0, &cancelled)
            .is_err()
    );
}

#[test]
fn coalesced_delta_and_full_page_advance_without_replay() {
    let (root, session, previous, first) = large_delta_fixture(512);
    let second_file = root.path().join("data/second.jsonl");
    std::fs::write(&second_file, "{\"name\":\"later\"}\n").unwrap();
    session.repository.index().add_file(&second_file).unwrap();
    let current = session.repository.index().view().unwrap();
    assert!(current.revision() > first.revision());
    let spec = backend::QuerySpec::default();
    let token = CancellationToken::new();
    let all = current
        .search_added_since_with_cancellation(
            &previous,
            &backend::build_query(&spec).unwrap(),
            &token,
        )
        .unwrap()
        .unwrap();
    assert_eq!(all.result.hits.len(), 513);
    let (kind, page) =
        incremental_delta_page(&session, &previous, &current, &spec, 0, &token).unwrap();
    assert!(matches!(kind, GridRequestKind::Delta(0)));
    assert_eq!(page.hits.len(), backend::MAX_PAGE_ROWS);
    let mut ids = HashSet::new();
    assert!(
        page.hits
            .iter()
            .all(|hit| ids.insert(hit.record_id.clone()))
    );
    assert_eq!(
        page.counters.selected_payload_reads,
        backend::MAX_PAGE_ROWS as u64
    );

    let (mut controller, _) = DesktopController::new();
    controller.session = Some(session);
    controller.executed_view = Some(previous);
    controller.executed_spec = Some(spec);
    controller.current_draft = SearchDraft::default();
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = vec![None; backend::MAX_PAGE_ROWS];
    controller.provisional_page_ready = true;
    let mut updates = Vec::new();
    controller.start_incremental_refresh(&mut updates);
    assert_eq!(
        controller.executed_view.as_ref().unwrap().revision(),
        current.revision()
    );
    let event = controller
        .workers
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(
        matches!(event, WorkerEvent::Queried { kind: GridRequestKind::Delta(_), result, .. } if result.is_ok())
    );
    controller.start_incremental_refresh(&mut updates);
    assert!(controller.workers.try_recv().is_err());
}

#[test]
fn indexed_query_projects_metadata_with_its_repository_root() {
    let fixture = crate::backend::test_support::fixture();
    let view = fixture.session.repository.index().view().unwrap();
    let (sender, receiver) = mpsc::channel();
    let spec = backend::QuerySpec {
        filter: "year = 2024".into(),
        ..Default::default()
    };
    spawn_grid_page(
        fixture.session.clone(),
        view,
        spec,
        0,
        Vec::new(),
        Vec::new(),
        vec![
            "@file.size".into(),
            "@repository.root".into(),
            "@record.id".into(),
        ],
        CancellationToken::new(),
        sender,
        0,
        0,
        GridRequestKind::First,
    );
    let event = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    let WorkerEvent::Queried { result, .. } = event else {
        panic!("expected indexed result")
    };
    let (page, hits, _, _, _, _, _) = (*result).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        &page.rows[0].cells[..3],
        &[
            std::fs::metadata(fixture._root.path().join("a.json"))
                .unwrap()
                .len()
                .to_string(),
            fixture._root.path().display().to_string(),
            "path:a.json".into(),
        ]
    );
}

#[test]
fn file_catalogue_uses_pinned_file_ids_without_copied_paths() {
    let fixture = crate::backend::test_support::fixture();
    let view = fixture.session.repository.index().view().unwrap();
    let catalogue = indexed_file_catalogue(view, &fixture.session.collections);
    assert_eq!(catalogue.ids.len(), 3);
    assert_eq!(catalogue.filtered_ids(Some("files"), None).len(), 3);
    assert_eq!(catalogue.filtered_ids(Some("missing"), None).len(), 0);
    for id in catalogue.ids.iter().copied() {
        assert!(catalogue.view.catalogue_row(id).is_some());
    }
}

#[test]
fn indexed_directory_tree_follows_added_and_deleted_files() {
    let fixture = crate::backend::test_support::fixture();
    let index = fixture.session.repository.index();
    assert!(
        indexed_directories(&index.view().unwrap(), &fixture.session.collections)
            .all
            .is_empty()
    );
    let file = fixture._root.path().join("docs/nested/new.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"title":"New"}"#).unwrap();
    index.add_file(&file).unwrap();
    let view = index.view().unwrap();
    let directories = indexed_directories(&view, &fixture.session.collections);
    assert_eq!(directories.all, ["docs", "docs/nested"]);
    assert_eq!(
        directories.by_collection[&fixture.session.collections[0]],
        ["docs", "docs/nested"]
    );
    assert!(view.file_paths_for_collection("missing").next().is_none());
    let handle = view.file_handle_for_path(&file).unwrap();
    index.delete_file(handle).unwrap();
    assert!(
        indexed_directories(&index.view().unwrap(), &fixture.session.collections)
            .all
            .is_empty()
    );
}

#[test]
fn presentation_refresh_keeps_selection_after_preview_is_closed() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    let draft = SearchDraft::default();
    controller.session = Some(fixture.session);
    controller.current_draft = draft.clone();
    controller.executed_draft = Some(draft.clone());
    controller.selected_index = Some(798);

    let mut updates = Vec::new();
    controller.start_query_impl(draft, 0, false, true, &mut updates);

    assert_eq!(controller.selected_index, Some(798));
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::QueryStarted {
            retain_preview: true,
            ..
        }
    )));
}

#[test]
fn loaded_index_receives_check_policy_before_old_scheduler_retires() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let replacement =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 4;
    controller.session = Some(replacement.clone());
    controller.requested_check = (true, 60);
    controller.transfer_check_policy(replacement.clone(), Some(original.clone()));
    let event = controller
        .workers
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(matches!(
        event,
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 4,
            enabled: true,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        replacement
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&replacement).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

#[test]
fn disabled_check_policy_retires_old_scheduler_on_replacement() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let replacement =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 9;
    controller.session = Some(replacement.clone());
    controller.requested_check = (false, 37);
    controller.transfer_check_policy(replacement.clone(), Some(original.clone()));
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 9,
            enabled: false,
            interval_seconds: 37,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        !replacement
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&replacement).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

#[test]
fn rapid_replacements_and_toggle_keep_only_latest_scheduler() {
    let fixture = crate::backend::test_support::fixture();
    let original = fixture.session.clone();
    let saved = tempfile::tempdir().unwrap();
    let path = saved.path().join("index.snapshot");
    backend::index_lifecycle::save_index(&original, &path, false).unwrap();
    backend::index_lifecycle::set_periodic_check(&original, Some(Duration::from_secs(60))).unwrap();
    let first =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let second =
        backend::index_lifecycle::load_index(&original, &path, &CancellationToken::new()).unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.open_generation = 11;
    let gate = Arc::clone(&controller.automatic_check_gate);
    let held = gate.lock().unwrap();
    controller.session = Some(first.clone());
    controller.requested_check = (true, 60);
    controller.transfer_check_policy(first.clone(), Some(original.clone()));
    controller.set_automatic_check(true, 90, &mut Vec::new());
    controller.session = Some(second.clone());
    controller.open_generation += 1;
    controller.transfer_check_policy(second.clone(), Some(first.clone()));
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticCheckChanged {
            open_generation: 12,
            enabled: true,
            interval_seconds: 90,
            result: Ok(()),
            ..
        }
    ));
    assert!(
        second
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !first
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    assert!(
        !original
            .repository
            .index()
            .periodic_reconciliation_enabled()
            .unwrap()
    );
    backend::index_lifecycle::shutdown(&second).unwrap();
    backend::index_lifecycle::shutdown(&first).unwrap();
    backend::index_lifecycle::shutdown(&original).unwrap();
}

#[test]
fn completion_uses_bounded_postings_and_keeps_typed_literals() {
    use objekte_search_core::model::LogicalType;
    use objekte_search_core::reference::FieldDefinition;
    use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions};
    use objekte_search_core::source::SourceFormat;

    let fixture = crate::backend::test_support::fixture();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("items")).unwrap();
    std::fs::write(root.path().join("items/a.json"), r#"{"name":"Alpha"}"#).unwrap();
    std::fs::write(root.path().join("items/b.json"), r#"{"name":"Beta"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("files", "items", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repository =
        Repository::open_with_options(root.path(), vec![definition], RepositoryOptions::default())
            .unwrap();
    let mut session = fixture.session;
    let index = objekte_search_core::lifecycle::Index::from_repository(repository).unwrap();
    session.repository =
        Arc::new(objekte_search_core::lifecycle_caller::IndexedRepository::new(index).unwrap());
    session.collections = vec!["files".into()];
    let (values, partial, visited) = posting_completions(
        &session,
        &CompletionRequest {
            origin: CompletionOrigin::Filter,
            revision: 1,
            collection: Some("files".into()),
            field: "name".into(),
            prefix: "Al".into(),
        },
        &CancellationToken::new(),
    );
    assert_eq!(values, [(r#""Alpha""#.into(), r#""Alpha""#.into())]);
    assert!(!partial);
    assert!(visited <= 20);
}

#[test]
fn declared_enum_completes_without_a_posting_dictionary() {
    use objekte_cli::arguments::Arguments;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), r#"{"status":"Ready"}"#).unwrap();
    let schema = root.path().join("schema.json");
    std::fs::write(
        &schema,
        r#"{"type":"object","properties":{"status":{"type":"string","enum":["Ready","Paused"]}}}"#,
    )
    .unwrap();
    let session = backend::open(
        &OpenSpec {
            storage_policy: None,
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                schema: Some(schema),
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let (values, _, visited) = posting_completions(
        &session,
        &CompletionRequest {
            origin: CompletionOrigin::Filter,
            revision: 1,
            collection: Some("files".into()),
            field: "status".into(),
            prefix: "Re".into(),
        },
        &CancellationToken::new(),
    );
    assert_eq!(values, [(r#""Ready""#.into(), r#""Ready""#.into())]);
    assert_eq!(visited, 0);
}

#[test]
fn bounded_batch_status_extraction_skips_large_hits() {
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("events.ndjson");
    let huge = "x".repeat(32 * 1024);
    std::fs::write(&output, format!(
        "{{\"type\":\"query_start\",\"query_id\":1,\"line\":4,\"input\":\"a = 1\"}}\n{{\"type\":\"hit\",\"data\":\"{huge}\"}}\n{{\"type\":\"query_error\",\"query_id\":1,\"error\":\"invalid\"}}\n"
    )).unwrap();
    let rows = batch_rows(&output).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].line, 4);
    assert_eq!(rows[0].query, "a = 1");
    assert_eq!(rows[0].error.as_deref(), Some("invalid"));
}

#[test]
fn cancelled_replacement_open_invalidates_late_loader_without_losing_active_draft() {
    let (mut controller, _) = DesktopController::new();
    let active = OpenDraft {
        paths: "/tmp/active".into(),
        ..OpenDraft::default()
    };
    let attempted = OpenDraft {
        paths: "/tmp/attempted".into(),
        ..OpenDraft::default()
    };
    controller.open_draft = Some(active.clone());
    controller.opening_draft = Some(attempted);
    controller.open_generation = 7;
    controller.pending_open_generation = 12;
    let mut updates = Vec::new();
    controller.cancel_open(&mut updates);
    assert_eq!(controller.open_generation, 7);
    assert_eq!(controller.pending_open_generation, 13);
    assert!(controller.opening_draft.is_none());
    assert_eq!(controller.open_draft, Some(active));
    assert!(controller.open_cancellation.is_cancelled());
    assert!(!controller.cancellation.is_cancelled());
    assert!(
        matches!(updates.as_slice(), [UiUpdate::Status(message)] if message == "Open cancelled")
    );
}

#[test]
fn publication_changes_invalidate_results_once_but_noop_checks_do_not() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    let view = fixture.session.repository.index().view().unwrap();
    let initial = Publication {
        instance_id: view.instance_id(),
        revision: view.revision(),
    };
    controller.observed_publication = Some(initial);
    controller.executed_view = Some(view);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.session = Some(fixture.session);
    let mut updates = Vec::new();
    controller.poll_publication(&mut updates);
    assert!(updates.is_empty());
    let report = backend::index_lifecycle::check_for_changes(controller.session.as_ref().unwrap())
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(report.added + report.updated + report.removed, 0);
    controller.poll_publication(&mut updates);
    assert!(!controller.results_outdated);
    assert!(updates.is_empty());

    std::fs::write(fixture._root.path().join("new.json"), r#"{"title":"New"}"#).unwrap();
    backend::index_lifecycle::check_for_changes(controller.session.as_ref().unwrap())
        .unwrap()
        .wait()
        .unwrap();
    controller.poll_publication(&mut updates);
    assert!(controller.results_outdated);
    assert!(controller.executed_view.is_none());
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::FieldChoices(_), UiUpdate::ResultsOutdated(true)]
    ));
    updates.clear();
    controller.poll_publication(&mut updates);
    assert!(updates.is_empty());
}

#[test]
fn manual_close_has_no_implicit_index_save() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    let mut updates = Vec::new();
    controller.request_close(&mut updates);
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::ClosingIndex { error: None }]
    ));
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::IndexShutdown { result: Ok(()), .. }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
}

#[test]
fn managed_index_panel_reports_storage_and_close_flushes() {
    use objekte_search_core::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), r#"{"title":"Alpha"}"#).unwrap();
    let session = backend::open(
        &OpenSpec {
            storage_policy: Some(IndexStoragePolicy {
                location: IndexStorageLocation::RepositoryDirectory,
                residency: IndexResidency::OnDemand,
            }),
            args: objekte_cli::arguments::Arguments {
                path: vec![root.path().to_path_buf()],
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(session);
    controller.open_draft = Some(OpenDraft {
        paths: root.path().display().to_string(),
        index_storage: StorageChoice::Repository,
        ..OpenDraft::default()
    });
    let mut updates = Vec::new();
    let status = controller.session.as_ref().unwrap().repository.index().status().unwrap();
    controller.update_index_ui(&status, &mut updates);
    assert_eq!(controller.index_ui.storage_location, "Repository folder");
    assert_eq!(controller.index_ui.residency, "Searchable on demand");
    assert!(controller.index_ui.synchronization.starts_with("Synchronized generation "));
    assert!(controller.index_ui.managed_directory.is_some());
    updates.clear();
    controller.request_close(&mut updates);
    assert!(matches!(controller.workers.recv_timeout(Duration::from_secs(5)).unwrap(),
        WorkerEvent::IndexShutdown { result: Ok(()), .. }));
}

#[test]
fn pending_automatic_save_enable_cannot_write_after_manual_close() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.index_ui.automatic_save_available = true;
    let save_gate = Arc::clone(&controller.automatic_save_gate);
    let held = save_gate.lock().unwrap();
    let mut updates = Vec::new();
    controller.set_automatic_save(true, &mut updates);
    controller.request_close(&mut updates);
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::IndexShutdown { result: Ok(()), .. }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
    assert_eq!(
        backend::index_lifecycle::status(controller.session.as_ref().unwrap())
            .unwrap()
            .saved_revision,
        None
    );
}

#[test]
fn superseded_automatic_save_enable_cannot_run_after_disable() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.index_ui.automatic_save_available = true;
    let save_gate = Arc::clone(&controller.automatic_save_gate);
    let held = save_gate.lock().unwrap();
    controller.set_automatic_save(true, &mut Vec::new());
    controller.set_automatic_save(false, &mut Vec::new());
    drop(held);
    assert!(matches!(
        controller
            .workers
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        WorkerEvent::AutomaticSaveChanged {
            enabled: false,
            result: Ok(()),
            ..
        }
    ));
    assert!(!fixture._root.path().join(".objekte").exists());
    backend::index_lifecycle::shutdown(controller.session.as_ref().unwrap()).unwrap();
}

#[test]
fn unchanged_index_status_does_not_redraw_the_workspace() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.open_draft = Some(OpenDraft::default());
    controller.opening_draft = Some(OpenDraft {
        index_storage: crate::ui::model::StorageChoice::Custom,
        index_directory: "/tmp/pending-indexes".into(),
        ..OpenDraft::default()
    });
    let status = controller
        .session
        .as_ref()
        .unwrap()
        .repository
        .index()
        .status()
        .unwrap();
    let mut updates = Vec::new();
    controller.update_index_ui(&status, &mut updates);
    assert!(matches!(updates.as_slice(), [UiUpdate::IndexStatus(_)]));
    assert_eq!(controller.index_ui.storage_location, "Memory only");
    assert_eq!(
        controller.index_ui.synchronization,
        "No index files are read or written"
    );

    updates.clear();
    controller.update_index_ui(&status, &mut updates);
    assert!(updates.is_empty());

    controller.index_ui.check = IndexTaskState::Succeeded("Files checked".into());
    controller.update_index_ui(&status, &mut updates);
    assert!(matches!(updates.as_slice(), [UiUpdate::IndexStatus(_)]));
}

#[test]
fn invalid_filter_does_not_start_query_or_replace_visible_page() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.selected_index = Some(11);
    let mut updates = Vec::new();
    controller.start_query(
        SearchDraft {
            structured: "year >=".into(),
            filter_enabled: true,
            ..SearchDraft::default()
        },
        0,
        &mut updates,
    );
    assert_eq!(controller.selected_index, Some(11));
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::FilterValidation {
            state: FilterValidationState::Invalid { .. },
            ..
        }
    )));
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, UiUpdate::QueryStarted { .. }))
    );
}

#[test]
fn main_query_change_preserves_tab_context_and_close_cancels_its_work() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        1,
        &CancellationToken::new(),
    )
    .unwrap();
    let hit = &page.hits[0];
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys.push(Some(RecordKey {
        collection: hit.collection.clone(),
        id: hit.record_id.clone(),
    }));
    let mut updates = Vec::new();
    controller.open_record(0, false, &mut updates);
    assert_eq!(controller.tabs.len(), 1);
    let tab_id = controller.tabs[0].id;
    let token = controller.tabs[0].cancellation.clone();
    controller.start_query(
        SearchDraft {
            full_text: "different".into(),
            ..SearchDraft::default()
        },
        0,
        &mut updates,
    );
    assert_eq!(controller.tabs[0].context.draft, SearchDraft::default());
    assert!(!token.is_cancelled());
    controller.close_tab(tab_id, &mut updates);
    assert!(controller.tabs.is_empty());
    assert!(token.is_cancelled());
}

#[test]
fn row_preview_reuses_only_its_own_slot_and_explicit_tabs_stay_pinned() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = page
        .hits
        .iter()
        .map(|hit| {
            Some(RecordKey {
                collection: hit.collection.clone(),
                id: hit.record_id.clone(),
            })
        })
        .collect();
    let mut updates = Vec::new();
    controller.open_record(0, false, &mut updates);
    let first_id = controller.active_tab.unwrap();
    assert!(!controller.tabs[0].pinned);
    controller.open_record(1, false, &mut updates);
    assert_eq!(controller.active_tab, Some(first_id));
    assert_eq!(controller.tabs.len(), 1);
    controller.open_record(1, true, &mut updates);
    assert_eq!(controller.tabs.len(), 2);
    let pinned_id = controller.active_tab.unwrap();
    assert_ne!(pinned_id, first_id);
    assert!(
        controller
            .tabs
            .iter()
            .find(|tab| tab.id == pinned_id)
            .unwrap()
            .pinned
    );

    controller.open_record(0, false, &mut updates);
    assert_eq!(controller.active_tab, Some(first_id));
    assert_eq!(controller.tabs.len(), 2);
    assert!(
        controller
            .tabs
            .iter()
            .find(|tab| tab.id == pinned_id)
            .unwrap()
            .pinned
    );
    controller.pin_preview(&mut updates);
    assert!(controller.tabs.iter().all(|tab| tab.pinned));
    assert!(updates.iter().any(|update| matches!(
        update,
        UiUpdate::Tabs { tabs, active: Some(id) }
            if *id == first_id && tabs.iter().find(|tab| tab.id == first_id).unwrap().pinned
    )));
}

#[test]
fn activating_record_tab_selects_matching_result_or_clears_old_selection() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = page
        .hits
        .iter()
        .map(|hit| {
            Some(RecordKey {
                collection: hit.collection.clone(),
                id: hit.record_id.clone(),
            })
        })
        .collect();
    let mut updates = Vec::new();
    controller.open_record(0, false, &mut updates);
    let first_id = controller.active_tab.unwrap();
    controller.open_record(1, true, &mut updates);
    let second_id = controller.active_tab.unwrap();

    updates.clear();
    controller.activate_tab(first_id, &mut updates);
    assert_eq!(controller.selected_index, Some(0));
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::RowSelection(Some(0))))
    );

    updates.clear();
    controller.close_tab(first_id, &mut updates);
    assert_eq!(controller.active_tab, None);
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::Tabs { active: None, .. }))
    );
    controller.activate_tab(second_id, &mut updates);
    assert_eq!(controller.active_tab, Some(second_id));
    assert_eq!(controller.selected_index, Some(1));
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::RowSelection(Some(1))))
    );

    controller.keys.clear();
    updates.clear();
    controller.activate_tab(second_id, &mut updates);
    assert_eq!(controller.selected_index, None);
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::RowSelection(None)))
    );
}

#[test]
fn active_record_selection_follows_reordered_and_appended_results() {
    let fixture = crate::backend::test_support::fixture();
    let page = backend::query_page(
        &fixture.session,
        &backend::QuerySpec::default(),
        0,
        10,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(page.hits.len() >= 2);
    let first = RecordKey {
        collection: page.hits[0].collection.clone(),
        id: page.hits[0].record_id.clone(),
    };
    let second = RecordKey {
        collection: page.hits[1].collection.clone(),
        id: page.hits[1].record_id.clone(),
    };
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.keys = vec![Some(first.clone()), Some(second.clone())];
    let mut updates = Vec::new();
    controller.open_record(1, false, &mut updates);

    // A new query may load the same record at a different position.
    controller.keys = vec![Some(second.clone()), Some(first.clone())];
    updates.clear();
    controller.sync_active_tab_row_selection_in(0, &mut updates);
    assert_eq!(controller.selected_index, Some(0));
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::RowSelection(Some(0))))
    );

    // Selection stays clear until the page containing that record arrives.
    controller.keys = vec![Some(first), None];
    updates.clear();
    controller.sync_active_tab_row_selection_in(0, &mut updates);
    assert_eq!(controller.selected_index, None);
    controller.keys[1] = Some(second);
    updates.clear();
    controller.sync_active_tab_row_selection_in(1, &mut updates);
    assert_eq!(controller.selected_index, Some(1));
}

#[test]
fn opening_record_outside_results_clears_previous_row_selection() {
    let fixture = crate::backend::test_support::fixture();
    let view = fixture.session.repository.index().view().unwrap();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.executed_view = Some(view);
    controller.executed_spec = Some(backend::QuerySpec::default());
    controller.executed_draft = Some(SearchDraft::default());
    controller.selected_index = Some(0);
    let mut updates = Vec::new();
    controller.open_canonical_outside_results(
        RecordKey {
            collection: "files".into(),
            id: "missing-record".into(),
        },
        true,
        &mut updates,
    );
    assert_eq!(controller.selected_index, None);
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, UiUpdate::RowSelection(None)))
    );
}

#[test]
fn invalid_current_request_cannot_export_previous_results() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.current_draft = SearchDraft {
        structured: "year >=".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let path = fixture._root.path().join("should-not-export.json");
    let mut updates = Vec::new();
    controller.start_export(
        ExportFormat::ResultsJson,
        path.display().to_string(),
        &mut updates,
    );
    assert!(!path.exists());
    assert!(matches!(
        updates.as_slice(),
        [UiUpdate::Error {
            field: ErrorField::Export,
            ..
        }]
    ));
}

#[test]
fn export_uses_current_valid_draft_before_debounce_runs() {
    let fixture = crate::backend::test_support::fixture();
    let (mut controller, _) = DesktopController::new();
    controller.session = Some(fixture.session);
    controller.current_draft = SearchDraft {
        structured: "year >= 2024".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("current.json");
    let mut updates = Vec::new();
    controller.start_export(
        ExportFormat::ResultsJson,
        path.display().to_string(),
        &mut updates,
    );
    for _ in 0..100 {
        if path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(result["query"]["filter"], "year >= 2024");
    assert_eq!(result["total"], 1);
}
