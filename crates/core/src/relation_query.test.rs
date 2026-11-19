use std::path::PathBuf;
use std::sync::Arc;

use crate::model::LogicalType;
use crate::query::{CaseSensitivity, FtsMode, Query};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};
use crate::schema::{FieldDefinition, Schema};
use crate::source::SourceFormat;

#[test]
fn merged_relations_resolve_replacement_targets_and_preserve_unknown() {
    use crate::construction::batch::SourceFileInput;
    use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
    use crate::construction::segment_admission::SegmentAdmission;
    use crate::index::segment::IndexedCollectionSegment;
    use crate::index::segment::PublishedSegment;
    use crate::inventory::{ActiveRecords, SourceRevision};
    use crate::lifecycle::IndexRevision;
    use crate::lifecycle::catalog::FileCatalog;
    use crate::model::{CollectionId, FileId};

    fn stage(
        repo: &Repository,
        root: &std::path::Path,
        collection_index: usize,
        file_id: FileId,
        relative_path: &str,
    ) -> IndexedCollectionSegment {
        let collection = &repo.collections()[collection_index];
        let path = root.join(relative_path);
        let metadata = std::fs::metadata(&path).unwrap();
        let prepared = prepare_file_build(
            root,
            SourceFileInput {
                id: file_id,
                relative_path: relative_path.into(),
                revision: SourceRevision::from_metadata(&metadata).unwrap(),
                created_ns: metadata
                    .created()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128),
            },
            &[FilePreparationBinding {
                collection: CollectionId(collection_index as u32),
                definition: &collection.definition,
                source: &collection.source,
            }],
            repo.options().bigram_alphabet,
            repo.options().inference,
            repo.collections().len(),
        )
        .unwrap();
        let mut admission =
            SegmentAdmission::new(CollectionId(collection_index as u32), &collection.source);
        admission
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        admission
            .finish()
            .into_indexed(
                &collection.definition,
                repo.options().bigram_alphabet,
                repo.options().inference,
                repo.collections().len(),
                &pool,
            )
            .unwrap()
    }

    let (root, definitions) = fixture();
    let repo = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions,
            RepositoryOptions {
                ..Default::default()
            },
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    let mut active = repo
        .collections()
        .iter()
        .map(|collection| ActiveRecords::all(collection.generation.records.len()).unwrap())
        .collect::<Vec<_>>();
    let old_a = repo.collections()[0].id_lookup["A"];
    let old_a_file = repo.collections()[0].generation.records[old_a.0 as usize].file;
    active[0] = active[0].retired(&[old_a]).unwrap();
    let empty_segments = vec![Arc::new(Vec::new()); repo.collections().len()];
    let catalog = FileCatalog::from_repository(Arc::clone(&repo))
        .unwrap()
        .retired(old_a_file)
        .unwrap();
    let missing = crate::query_execution::runner::search_segmented(
        &repo,
        &query(
            "mentions",
            Some("!(entry.title = 'Hymn')"),
            None,
            None,
            false,
        ),
        &active,
        &empty_segments,
        &catalog,
        IndexRevision(2),
    )
    .unwrap();
    assert!(!missing.hits.iter().any(|hit| hit.record_id == "m1"));
    std::fs::remove_file(root.join("entries/a.json")).unwrap();

    std::fs::write(
        root.join("entries/new-a.json"),
        r#"{"id":"A","title":"New Hymn"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/new-m.json"),
        r#"{"id":"m8","entry":"A","title":"New source"}"#,
    )
    .unwrap();
    let next_file = repo.files().files.len() as u32;
    let target = stage(&repo, &root, 0, FileId(next_file), "entries/new-a.json");
    let source = stage(
        &repo,
        &root,
        1,
        FileId(next_file + 1),
        "mentions/new-m.json",
    );
    let catalog = catalog
        .appended(target.files[0].entry.clone(), None, None)
        .unwrap()
        .appended(source.files[0].entry.clone(), None, None)
        .unwrap();
    let segments = vec![
        Arc::new(vec![Arc::new(PublishedSegment {
            id: crate::index::segment::SegmentId::new(1),
            active: ActiveRecords::all(target.records.len()).unwrap(),
            index: Arc::new(target),
        })]),
        Arc::new(vec![Arc::new(PublishedSegment {
            id: crate::index::segment::SegmentId::new(2),
            active: ActiveRecords::all(source.records.len()).unwrap(),
            index: Arc::new(source),
        })]),
    ];
    let find = |request: Query| {
        crate::query_execution::runner::search_segmented(
            &repo,
            &request,
            &active,
            &segments,
            &catalog,
            IndexRevision(3),
        )
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record_id)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        find(query(
            "mentions",
            Some("entry.title = 'New Hymn'"),
            None,
            None,
            false
        )),
        ["m1", "m8"]
    );
    assert_eq!(
        find(query(
            "mentions",
            None,
            Some("New"),
            Some("entry.title"),
            false
        )),
        ["m1", "m8"]
    );
    assert!(
        find(query(
            "mentions",
            Some("!(entry.title = 'New Hymn')"),
            None,
            None,
            false
        ))
        .contains(&"m2".to_owned())
    );
    assert_eq!(
        find(query(
            "entries",
            Some("@backlinks.mentions.title ?~ 'Festival'"),
            None,
            None,
            false
        )),
        ["A"]
    );
    let outgoing = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo,
        "mentions",
        &["entry".into()],
        "m8",
        0,
        1,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(outgoing.source.segment_ordinal, Some(2));
    assert_eq!(outgoing.total_occurrences, 1);
    assert_eq!(outgoing.references[0].resolved.as_ref().unwrap().id, "A");
    assert_eq!(
        outgoing.references[0]
            .resolved
            .as_ref()
            .unwrap()
            .segment_ordinal,
        Some(1)
    );
    let nested = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo,
        "mentions",
        &["items".into(), "entry".into()],
        "m1",
        0,
        1,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(nested.total_occurrences, 2);
    assert_eq!(nested.next_offset, Some(1));
    assert_eq!(nested.references[0].positions, [0]);
    assert_eq!(
        nested.references[0]
            .resolved
            .as_ref()
            .unwrap()
            .segment_ordinal,
        Some(1)
    );
    let null = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo,
        "mentions",
        &["entry".into()],
        "m4",
        0,
        1,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(
        null.presence,
        crate::relation_inspection::RelationPresence::Present
    );
    assert!(null.contains_null);
    assert_eq!(null.total_occurrences, 0);
    let backlinks = crate::relation_inspection::relation_backlink_page_segmented(
        &repo,
        "entries",
        "mentions",
        "A",
        0,
        1,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(backlinks.total_sources, 2);
    assert_eq!(backlinks.next_offset, Some(1));
    assert_eq!(backlinks.sources[0].id, "m1");
    let backlink_tail = crate::relation_inspection::relation_backlink_page_segmented(
        &repo,
        "entries",
        "mentions",
        "A",
        1,
        1,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(backlink_tail.sources[0].id, "m8");
    assert_eq!(backlink_tail.sources[0].segment_ordinal, Some(2));
    let graph = crate::relation_inspection::collection_graph_segmented(
        &repo,
        IndexRevision(3),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(graph.nodes[0].record_count, 2);
    assert_eq!(graph.nodes[1].generation, 3);
    let mentions_edge = graph
        .edges
        .iter()
        .find(|edge| edge.reverse_alias.as_deref() == Some("mentions"))
        .unwrap();
    assert_eq!(mentions_edge.occurrence_count, 4);
    assert_eq!(mentions_edge.unresolved_occurrence_count, 1);
    let mut retired_source = (*segments[1][0]).clone();
    retired_source.active = retired_source
        .active
        .retired(&[crate::model::RecordSlot(0)])
        .unwrap();
    let without_source = vec![
        Arc::clone(&segments[0]),
        Arc::new(vec![Arc::new(retired_source)]),
    ];
    let source_retired_backlinks = crate::relation_inspection::relation_backlink_page_segmented(
        &repo,
        "entries",
        "mentions",
        "A",
        0,
        10,
        IndexRevision(4),
        &active,
        &without_source,
        &catalog,
    )
    .unwrap();
    assert_eq!(source_retired_backlinks.total_sources, 1);
    assert_eq!(source_retired_backlinks.sources[0].id, "m1");
    let source_retired_graph = crate::relation_inspection::collection_graph_segmented(
        &repo,
        IndexRevision(4),
        &active,
        &without_source,
        &catalog,
    )
    .unwrap();
    assert_eq!(
        source_retired_graph
            .edges
            .iter()
            .find(|edge| { edge.reverse_alias.as_deref() == Some("mentions") })
            .unwrap()
            .occurrence_count,
        3
    );
    assert_eq!(
        find(query(
            "entries",
            Some("@backlinks.mentions.title ?~ 'New source'"),
            None,
            None,
            false
        )),
        ["A"]
    );
    let mut sorted = query("mentions", None, None, None, false);
    sorted.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "entry.title".into(),
            binding: Some("lower".into()),
            reducer: None,
        },
        false,
    )];
    let ordered = find(sorted);
    let at = |id: &str| ordered.iter().position(|found| found == id).unwrap();
    assert!(at("m2") < at("m1"));
    assert!(at("m1") < at("m8"));
    let mut first = query("mentions", None, None, None, false);
    first.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::execution_options::SortReducer::First),
        },
        false,
    )];
    let first_order = find(first);
    let first_at = |id: &str| first_order.iter().position(|found| found == id).unwrap();
    assert!(first_at("m2") < first_at("m1"));
    let mut last = query("mentions", None, None, None, false);
    last.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::execution_options::SortReducer::Last),
        },
        false,
    )];
    let last_order = find(last);
    let last_at = |id: &str| last_order.iter().position(|found| found == id).unwrap();
    assert!(last_at("m1") < last_at("m2"));
    let mut backlink_sort = query("entries", None, None, None, false);
    backlink_sort.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "@backlinks.mentions.title".into(),
            binding: None,
            reducer: Some(crate::execution_options::SortReducer::Min),
        },
        false,
    )];
    assert_eq!(find(backlink_sort), ["A", "B"]);
    let mut retired_target = (*segments[0][0]).clone();
    retired_target.active = retired_target
        .active
        .retired(&[crate::model::RecordSlot(0)])
        .unwrap();
    let retired_segments = vec![
        Arc::new(vec![Arc::new(retired_target)]),
        Arc::clone(&segments[1]),
    ];
    let retired = |request: Query| {
        crate::query_execution::runner::search_segmented(
            &repo,
            &request,
            &active,
            &retired_segments,
            &catalog,
            IndexRevision(4),
        )
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| hit.record_id)
        .collect::<Vec<_>>()
    };
    let negated = retired(query(
        "mentions",
        Some("!(entry.title = 'New Hymn')"),
        None,
        None,
        false,
    ));
    assert!(!negated.contains(&"m1".to_owned()));
    assert!(!negated.contains(&"m8".to_owned()));
    assert!(
        retired(query(
            "mentions",
            None,
            Some("New"),
            Some("entry.title"),
            false
        ))
        .is_empty()
    );
    let unresolved = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo,
        "mentions",
        &["entry".into()],
        "m8",
        0,
        1,
        IndexRevision(4),
        &active,
        &retired_segments,
        &catalog,
    )
    .unwrap();
    assert!(unresolved.references[0].resolved.is_none());
    assert!(
        crate::relation_inspection::relation_backlink_page_segmented(
            &repo,
            "entries",
            "mentions",
            "A",
            0,
            1,
            IndexRevision(4),
            &active,
            &retired_segments,
            &catalog,
        )
        .is_err()
    );
    let retired_graph = crate::relation_inspection::collection_graph_segmented(
        &repo,
        IndexRevision(4),
        &active,
        &retired_segments,
        &catalog,
    )
    .unwrap();
    let retired_edge = retired_graph
        .edges
        .iter()
        .find(|edge| edge.reverse_alias.as_deref() == Some("mentions"))
        .unwrap();
    assert_eq!(retired_edge.unresolved_occurrence_count, 3);
    std::fs::remove_file(root.join("entries/new-a.json")).unwrap();
    let unavailable_target = crate::query_execution::runner::search_segmented(
        &repo,
        &query(
            "mentions",
            Some("!(entry.title = 'New Hymn')"),
            None,
            None,
            false,
        ),
        &active,
        &segments,
        &catalog,
        IndexRevision(3),
    )
    .unwrap();
    assert_eq!(unavailable_target.count, None);
    assert!(!unavailable_target.counters.match_count_complete);
    assert_eq!(
        unavailable_target.counters.source_diagnostics[0].kind,
        crate::measurement::SourceUnavailableKind::Missing
    );
    assert!(
        !unavailable_target
            .hits
            .iter()
            .any(|hit| hit.record_id == "m1" || hit.record_id == "m8")
    );
    let mut unavailable_sort = query("mentions", None, None, None, false);
    unavailable_sort.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "entry.title".into(),
            binding: Some("lower".into()),
            reducer: None,
        },
        false,
    )];
    let unavailable_sort = crate::query_execution::runner::search_segmented(
        &repo,
        &unavailable_sort,
        &active,
        &segments,
        &catalog,
        IndexRevision(3),
    )
    .unwrap();
    assert_eq!(unavailable_sort.count, None);
    assert!(
        !unavailable_sort
            .hits
            .iter()
            .any(|hit| hit.record_id == "m1" || hit.record_id == "m8")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn merged_relation_walker_matches_static_reference_semantics() {
    use crate::inventory::ActiveRecords;
    use crate::lifecycle::IndexRevision;
    use crate::lifecycle::catalog::FileCatalog;

    let (root, definitions) = fixture();
    let repo = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions,
            RepositoryOptions {
                ..Default::default()
            },
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    let active = repo
        .collections()
        .iter()
        .map(|collection| ActiveRecords::all(collection.generation.records.len()).unwrap())
        .collect::<Vec<_>>();
    let segments = vec![Arc::new(Vec::new()); repo.collections().len()];
    let catalog = FileCatalog::from_repository(Arc::clone(&repo)).unwrap();
    for request in [
        query("mentions", Some("entry.title = 'Hymn'"), None, None, false),
        query(
            "mentions",
            Some("contributors[0].title = 'Hymn'"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            Some("entry.related_entry.title = 'Festival'"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            Some("entry.title = local_title"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            Some("!(entry.title = 'Hymn')"),
            None,
            None,
            false,
        ),
        query(
            "entries",
            Some("@backlinks.mentions.title ?~ 'Festival'"),
            None,
            None,
            false,
        ),
        query("mentions", None, Some("Hymn"), Some("entry.title"), false),
        query(
            "entries",
            None,
            Some("Festival"),
            Some("@backlinks.mentions.title"),
            false,
        ),
    ] {
        let expected = repo.search_reference(&request).unwrap();
        let actual = crate::query_execution::runner::search_segmented(
            &repo,
            &request,
            &active,
            &segments,
            &catalog,
            IndexRevision(1),
        )
        .unwrap();
        assert_eq!(actual.count, expected.count, "{request:?}");
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|hit| (&hit.record_id, &hit.matched_fields))
                .collect::<Vec<_>>(),
            expected
                .hits
                .iter()
                .map(|hit| (&hit.record_id, &hit.matched_fields))
                .collect::<Vec<_>>(),
            "{request:?}"
        );
    }
    for (collection, path, reducer) in [
        ("mentions", "entry.title", None),
        (
            "mentions",
            "items.entry.title",
            Some(crate::execution_options::SortReducer::First),
        ),
        (
            "mentions",
            "items.entry.title",
            Some(crate::execution_options::SortReducer::Last),
        ),
        (
            "mentions",
            "contributors.title",
            Some(crate::execution_options::SortReducer::Min),
        ),
        (
            "entries",
            "@backlinks.mentions.title",
            Some(crate::execution_options::SortReducer::Min),
        ),
    ] {
        let mut request = query(collection, None, None, None, false);
        request.options.sort = vec![crate::execution_options::SortField::computed(
            crate::execution_options::SortExpression {
                path: path.into(),
                binding: None,
                reducer,
            },
            false,
        )];
        let expected = repo.search_reference(&request).unwrap();
        let actual = crate::query_execution::runner::search_segmented(
            &repo,
            &request,
            &active,
            &segments,
            &catalog,
            IndexRevision(1),
        )
        .unwrap();
        assert_eq!(actual.count, expected.count, "{path}");
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>(),
            expected
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>(),
            "{path}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
use crate::execution_options::ReturnMode;
use crate::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamError, StreamEvent, StreamOptions,
    StreamOutcome,
};
use crate::workload::WorkloadCase;

fn field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi,
            element_nullable: vec![false],
        })
        .unwrap();
}

#[test]
fn relation_work_limits_return_explicit_errors() {
    let mut work = super::RelationWork {
        edge_visits: super::MAX_QUERY_EDGE_VISITS,
        ..Default::default()
    };
    assert!(work.edge().unwrap_err().contains("edge visits"));
    let mut work = super::RelationWork {
        backlink_probe_visits: super::MAX_QUERY_BACKLINK_PROBES,
        ..Default::default()
    };
    assert!(
        work.backlink_probe()
            .unwrap_err()
            .contains("backlink probes")
    );
    let mut work = super::RelationWork {
        target_reads: super::MAX_QUERY_RECORD_READS,
        ..Default::default()
    };
    assert!(work.target_read(1).unwrap_err().contains("target reads"));
    let mut work = super::RelationWork {
        bytes_read: super::MAX_QUERY_RELATION_BYTES,
        ..Default::default()
    };
    assert!(work.source_read(1).unwrap_err().contains("source bytes"));
    let mut counters = crate::measurement::WorkCounters {
        relation_edge_visits: super::MAX_QUERY_EDGE_VISITS + 1,
        ..Default::default()
    };
    assert!(super::check_query_work_budget(&counters).is_err());
    counters.relation_edge_visits = 0;
    counters.relation_bytes_read = super::MAX_QUERY_RELATION_BYTES + 1;
    assert!(super::check_query_work_budget(&counters).is_err());
}

fn relation(schema: &mut Schema, path: &str, target: &str, many: bool, alias: &str) {
    schema
        .register_relation(RelationDefinition {
            path: vec![path.into()],
            target_collection: target.into(),
            cardinality: if many {
                RelationCardinality::Many
            } else {
                RelationCardinality::One
            },
            reverse_alias: Some(alias.into()),
        })
        .unwrap();
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

#[test]
fn lifecycle_relation_and_exact_local_id_use_direct_witness() {
    use crate::inventory::SourceRevision;

    let (root, definitions) = fixture();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    for filter in [
        "entry.title = 'Hymn' && @record.id = 'm1'",
        "@record.id = 'm1' && entry.title = 'Hymn'",
        "entry.title = 'Hymn' && @record.id = 'm2'",
    ] {
        let request = query("mentions", Some(filter), None, None, false);
        let expected = view.search_reference(&request).unwrap();
        let actual = view.search(&request).unwrap();
        assert_eq!(actual.count, expected.count, "{filter}");
        assert_eq!(actual.hits, expected.hits, "{filter}");
        assert_eq!(actual.counters.scanned_records, 0, "{filter}");
    }
    let target = root.join("entries/a.json");
    let metadata = std::fs::metadata(&target).unwrap();
    std::fs::rename(&target, root.join("entries/hidden-a.json")).unwrap();
    std::fs::write(&target, vec![b'x'; metadata.len() as usize]).unwrap();
    std::fs::File::open(&target)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&target).unwrap()).unwrap(),
        SourceRevision::from_metadata(&metadata).unwrap(),
    );
    let mut bytes = query(
        "mentions",
        Some("entry.title = 'Hymn' && @record.id = 'm1'"),
        None,
        None,
        false,
    );
    bytes.options.return_mode = ReturnMode::Bytes;
    // Trusted search may fail while extracting externally corrupted bytes;
    // it must not turn that edit into an implicit index refresh.
    let stale = view.search(&bytes).unwrap_err();
    assert!(!stale.contains("stale source"));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_related_sort_uses_target_stored_column() {
    use crate::model::FieldPath;

    let (root, mut definitions) = fixture();
    definitions[0]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    let mut request = query(
        "mentions",
        Some("entry.title != 'Missing'"),
        None,
        None,
        false,
    );
    request.options.sort = vec![crate::execution_options::SortField::computed(
        crate::execution_options::SortExpression {
            path: "entry.title".into(),
            binding: None,
            reducer: None,
        },
        false,
    )];
    let expected = view.search_reference(&request).unwrap();
    let actual = view.search(&request).unwrap();
    assert_eq!(actual.count, expected.count);
    assert_eq!(actual.hits, expected.hits);
    assert_eq!(actual.counters.scanned_records, 0);
    assert_eq!(actual.counters.sort_target_reads, 0);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_direct_relation_count_and_stored_sort_pages_match_reference() {
    use crate::model::FieldPath;

    let (root, mut definitions) = fixture();
    definitions[1]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    for number in 0..80 {
        std::fs::write(
            root.join(format!("mentions/extra-{number:03}.json")),
            format!(
                r#"{{"id":"extra-{number:03}","entry":"A","title":"{:03}"}}"#,
                (number * 37) % 80,
            ),
        )
        .unwrap();
    }
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    for descending in [false, true] {
        for offset in [0, 17, 70] {
            for count in [false, true] {
                let mut request =
                    query("mentions", Some("entry.title = 'Hymn'"), None, None, false);
                request.options.sort = vec![crate::execution_options::SortField::simple(
                    "title", descending,
                )];
                request.options.offset = offset;
                request.options.limit = 5;
                request.options.count = count;
                let expected = view.search_reference(&request).unwrap();
                let actual = view.search(&request).unwrap();
                assert_eq!(actual.count, expected.count);
                assert_eq!(actual.hits, expected.hits);
                assert_eq!(actual.counters.scanned_records, 0);
                assert_eq!(actual.counters.files_opened, 0);
            }
        }
    }
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock() {
    use crate::execution_options::{SortExpression, SortField};
    use crate::model::FieldPath;
    use std::collections::HashMap;

    let (root, mut definitions) = fixture();
    definitions[1]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    std::fs::write(
        root.join("entries/a.json"),
        r#"{"id":"A","title":"Target"}"#,
    )
    .unwrap();
    std::fs::write(root.join("entries/b.json"), r#"{"id":"B","title":"Other"}"#).unwrap();
    std::fs::write(
        root.join("mentions/m1.json"),
        r#"{"id":"m1","entry":"A","title":"tie","local_title":"residual alpha"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.json"),
        r#"{"id":"m2","entry":"A","title":"retired old","local_title":"not selected"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m4.json"),
        r#"{"id":"m4","entry":"B","title":"zzz","local_title":"residual needle"}"#,
    )
    .unwrap();
    std::fs::remove_file(root.join("mentions/m3.json")).unwrap();

    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let before = index.view().unwrap();
    let m2 = before
        .file_handle_for_path(std::path::Path::new("mentions/m2.json"))
        .unwrap();
    std::fs::write(
        root.join("mentions/m2.json"),
        r#"{"id":"m2","entry":"A","title":"tie","local_title":"residual beta"}"#,
    )
    .unwrap();
    index
        .update_file(m2, &root.join("mentions/m2.json"))
        .unwrap();
    std::fs::write(
        root.join("mentions/m3.json"),
        r#"{"id":"m3","entry":"A","title":"tie","local_title":"residual gamma"}"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m3.json")).unwrap();

    let view = index.view().unwrap();
    let mut request = query(
        "mentions",
        Some(
            "local_title ~ $needle && (entry = 'A' || entry = 'B') && @file.modified >= @yearStart",
        ),
        Some("Target"),
        Some("entry.title"),
        false,
    );
    request.options.sort = vec![SortField::computed(
        SortExpression {
            path: "title".into(),
            binding: None,
            reducer: None,
        },
        false,
    )];
    request.options.limit = 2;
    request.options.count = true;
    request = request
        .with_clock(time::OffsetDateTime::from_unix_timestamp(946684800).unwrap())
        .unwrap();
    let parameters = HashMap::from([(
        "needle".into(),
        crate::model::Value::String("residual%".into()),
    )]);

    let reference = view
        .search_reference_with_parameters(&request, &parameters)
        .unwrap();
    assert_eq!(reference.count, Some(3));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert!(
        reference.hits[0]
            .matched_fields
            .iter()
            .any(|field| field == "entry.title")
    );
    assert!(
        reference.hits[0]
            .matched_fields
            .iter()
            .any(|field| field == "local_title")
    );
    assert!(
        reference.hits.iter().all(|hit| hit.record_id != "m4"),
        "source truth must reject the related-FTS false positive"
    );

    let optimized = view
        .search(&request.with_parameters(&parameters).unwrap())
        .unwrap();
    assert_eq!(optimized.count, reference.count);
    assert_eq!(optimized.hits, reference.hits);

    let mut unresolved = query(
        "mentions",
        Some("@file.modified >= @yearStart"),
        None,
        None,
        false,
    );
    unresolved.options.limit = 100;
    let smoke = view.search_reference(&unresolved).unwrap();
    assert!(smoke.count.unwrap_or_default() > 0);

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_reference_relations_ignore_perturbed_forward_and_backlink_contributions() {
    use crate::index::posting::ScalarKey;
    use crate::index::segment::PublishedSegment;
    use crate::model::CollectionId;
    use crate::relation::RelationId;

    let (root, definitions) = fixture();
    std::fs::write(root.join("entries/a.json"), r#"{"id":"A","title":"Hymn"}"#).unwrap();
    std::fs::write(root.join("entries/b.json"), r#"{"id":"B","title":"Other"}"#).unwrap();
    std::fs::write(
        root.join("mentions/m1.json"),
        r#"{"id":"m1","entry":"B","title":"base"}"#,
    )
    .unwrap();
    std::fs::remove_file(root.join("mentions/m2.json")).unwrap();
    std::fs::remove_file(root.join("mentions/m3.json")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.json"),
        r#"{"id":"m2","entry":"A","title":"segment"}"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m2.json")).unwrap();
    std::fs::write(
        root.join("mentions/m3.json"),
        r#"{"id":"m3","entry":"A","title":"segment"}"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m3.json")).unwrap();

    let pinned = index.view().unwrap();
    let repo = pinned.repository();
    let relation_index = repo.collections[1]
        .relations
        .iter()
        .position(|relation| relation.declaration.path == ["entry"])
        .expect("entry relation");
    let relation = RelationId {
        source: CollectionId(1),
        ordinal: u32::try_from(relation_index).unwrap(),
    };
    let relation_path = repo.collections[1].relations[relation_index]
        .declaration
        .path
        .clone();
    let source = CollectionId(1);
    let mut perturbed = pinned.0.segments.clone();
    let source_segments = pinned.0.segments[1]
        .iter()
        .map(|published| {
            let mut index = (*published.index).clone();
            index.relations[relation_index].remove_references_to_for_test("A");
            if let Some(posting) = index.postings.fields.get_mut(&relation_path) {
                posting.values.remove(&ScalarKey::Id("A".into()));
            }
            Arc::new(PublishedSegment {
                id: published.id,
                index: Arc::new(index),
                active: published.active.clone(),
            })
        })
        .collect::<Vec<_>>();
    perturbed[1] = Arc::new(source_segments);
    let prepare_before = crate::query::prepared::production_prepare_calls();
    let plan_before = crate::query_plan::operations::production_relation_plan_calls();
    let reference_forward_query =
        query("mentions", Some("entry.title = 'Hymn'"), None, None, false);
    let reference_forward_scan = crate::reference::relations::search_segmented(
        repo,
        &reference_forward_query,
        &pinned.0.active,
        &perturbed,
        &pinned.0.catalog,
        pinned.revision(),
    )
    .unwrap();
    assert_eq!(
        reference_forward_scan
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m3"]
    );
    let reference_backlink_query = query(
        "entries",
        Some("@backlinks.mentions.title = 'segment'"),
        None,
        None,
        false,
    );
    let reference_backlink_scan = crate::reference::relations::search_segmented(
        repo,
        &reference_backlink_query,
        &pinned.0.active,
        &perturbed,
        &pinned.0.catalog,
        pinned.revision(),
    )
    .unwrap();
    assert_eq!(
        reference_backlink_scan
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
    assert_eq!(
        crate::query::prepared::production_prepare_calls(),
        prepare_before
    );
    assert_eq!(
        crate::query_plan::operations::production_relation_plan_calls(),
        plan_before
    );
    let view =
        super::MergedRelationView::new(repo, &pinned.0.active, &perturbed, &pinned.0.catalog)
            .unwrap();
    let m2 = view
        .resolve_id(source, "m2")
        .unwrap()
        .expect("segmented m2 remains visible");
    let optimized_forward = view.forward_edges(m2, relation_index).unwrap();
    assert!(optimized_forward.0.is_empty());
    let reference_forward = view
        .reference_forward_edges(m2, relation, None)
        .unwrap()
        .expect("source-backed m2 value");
    assert_eq!(
        reference_forward
            .values
            .iter()
            .map(|(id, positions)| (id.as_str(), positions.as_slice()))
            .collect::<Vec<_>>(),
        [("A", &[][..])]
    );

    let optimized_backlinks = view.backlink_sources(relation, "A").unwrap();
    assert!(optimized_backlinks.is_empty());
    let reference_backlinks = view
        .reference_backlink_sources(relation, "A", None)
        .unwrap();
    let mut ids = reference_backlinks
        .iter()
        .map(|address| {
            view.record(*address)
                .unwrap()
                .unwrap()
                .record
                .id
                .0
                .to_string()
        })
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["m2", "m3"]);

    drop(view);
    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_parallel_relation_files_preserve_answers_and_missing_target_diagnostics() {
    use crate::streaming::{
        CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    };

    let (root, definitions) = fixture();
    let build = |workers| {
        crate::lifecycle::Index::start_build(
            &root,
            definitions.clone(),
            RepositoryOptions {
                query_workers: workers,
                ..RepositoryOptions::default()
            },
            crate::source::registry::SourceRegistry::builtin(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap()
    };
    let single = build(1);
    let parallel = build(4);
    let one = single.view().unwrap();
    let many = parallel.view().unwrap();
    let queries = [
        query(
            "mentions",
            Some("entry.title = 'Hymn'"),
            Some("Festival"),
            None,
            false,
        ),
        query(
            "mentions",
            None,
            Some("Hymn"),
            Some("items[0].entry.title"),
            false,
        ),
    ];
    for request in &queries {
        let expected = one.search(request).unwrap();
        let actual = many.search(request).unwrap();
        let reference = many.search_reference(request).unwrap();
        assert_eq!(actual.count, expected.count);
        assert_eq!(actual.hits, expected.hits);
        assert_eq!(actual.count, reference.count);
        assert_eq!(actual.hits, reference.hits);
        assert_eq!(actual.counters.files_opened, expected.counters.files_opened);
        assert_eq!(actual.counters.bytes_read, expected.counters.bytes_read);
        assert_eq!(
            actual.counters.relation_target_reads,
            expected.counters.relation_target_reads
        );
    }
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let context = ExecutionContext::new(
        crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits::default(),
        Some(cancellation),
    );
    assert_eq!(
        many.search_with_context(&queries[0], &context)
            .unwrap_err()
            .kind,
        ExecutionErrorKind::Cancelled,
    );
    std::fs::remove_file(root.join("entries/a.json")).unwrap();
    for request in &queries {
        let expected = one.search(request).unwrap();
        let actual = many.search(request).unwrap();
        assert_eq!(actual.count, expected.count);
        assert_eq!(actual.hits, expected.hits);
        assert_eq!(
            actual.counters.source_unavailable_events,
            expected.counters.source_unavailable_events
        );
        assert_eq!(
            actual.counters.match_count_complete,
            expected.counters.match_count_complete
        );
        assert_eq!(
            actual.counters.source_diagnostics,
            expected.counters.source_diagnostics
        );
        assert_eq!(actual.count, None);
        assert!(!actual.counters.match_count_complete);
        assert!(actual.counters.source_unavailable_events > 0);
    }
    single.shutdown().unwrap();
    parallel.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_parallel_direct_targets_match_serial_and_unavailable_fallback() {
    let (root, definitions) = fixture();
    for number in 0..24 {
        std::fs::write(
            root.join(format!("entries/extra-{number:03}.json")),
            format!(r#"{{"id":"E{number:03}","title":"Hymn {number}"}}"#),
        )
        .unwrap();
    }
    let build = |workers| {
        crate::lifecycle::Index::start_build(
            &root,
            definitions.clone(),
            RepositoryOptions {
                query_workers: workers,
                ..RepositoryOptions::default()
            },
            crate::source::registry::SourceRegistry::builtin(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap()
    };
    let single = build(1);
    let parallel = build(4);
    let one = single.view().unwrap();
    let many = parallel.view().unwrap();
    let queries = [
        query("mentions", Some("entry.title ~ 'Hymn'"), None, None, false),
        query("mentions", None, Some("Hymn.*"), Some("entry.title"), true),
    ];
    for request in &queries {
        let expected = one.search(request).unwrap();
        let actual = many.search(request).unwrap();
        let reference = many.search_reference(request).unwrap();
        assert_eq!(actual.count, expected.count);
        assert_eq!(actual.hits, expected.hits);
        assert_eq!(actual.count, reference.count);
        assert_eq!(actual.hits, reference.hits);
        assert_eq!(
            actual.counters.relation_target_reads,
            expected.counters.relation_target_reads
        );
        assert!(actual.counters.relation_target_reads > 1);
    }
    std::fs::remove_file(root.join("entries/a.json")).unwrap();
    for request in &queries {
        let expected = one.search(request).unwrap();
        let actual = many.search(request).unwrap();
        assert_eq!(actual.count, expected.count);
        assert_eq!(actual.hits, expected.hits);
        assert_eq!(
            actual.counters.source_unavailable_events,
            expected.counters.source_unavailable_events
        );
        assert_eq!(
            actual.counters.match_count_complete,
            expected.counters.match_count_complete
        );
        assert_eq!(
            actual.counters.source_diagnostics,
            expected.counters.source_diagnostics
        );
        assert_eq!(actual.count, None);
    }
    single.shutdown().unwrap();
    parallel.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (PathBuf, Vec<CollectionDefinition>) {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-query-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    for name in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (name, text) in [
        (
            "entries/a.json",
            r#"{"id":"A","title":"Hymn","alt_title":"Hymn","related_entry":"B"}"#,
        ),
        (
            "entries/b.json",
            r#"{"id":"B","title":"Festival","alt_title":"Other","related_entry":"A"}"#,
        ),
        (
            "mentions/m1.json",
            r#"{"id":"m1","entry":"A","items":[{"entry":"A"},{"entry":"B"}],"title":"Festival","local_title":"Hymn"}"#,
        ),
        (
            "mentions/m2.json",
            r#"{"id":"m2","entry":"B","items":[{"entry":"B"}],"title":"Hymn","local_title":"x"}"#,
        ),
        (
            "mentions/m3.json",
            r#"{"id":"m3","entry":"Z","title":"Dangling"}"#,
        ),
        (
            "mentions/m4.json",
            r#"{"id":"m4","entry":null,"title":"Null"}"#,
        ),
        ("mentions/m5.json", r#"{"id":"m5","title":"Missing"}"#),
        (
            "mentions/m6.json",
            r#"{"id":"m6","contributors":["A","B"],"title":"Both"}"#,
        ),
        (
            "mentions/m7.json",
            r#"{"id":"m7","contributors":[],"title":"Empty"}"#,
        ),
        ("mentions/broken.json", "{broken"),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }
    let mut entries = Schema::default();
    field(&mut entries, "id", LogicalType::Id, false);
    field(&mut entries, "title", LogicalType::String, false);
    field(&mut entries, "alt_title", LogicalType::String, false);
    field(&mut entries, "related_entry", LogicalType::Id, false);
    relation(
        &mut entries,
        "related_entry",
        "entries",
        false,
        "related_from",
    );
    let mut mentions = Schema::default();
    field(&mut mentions, "id", LogicalType::Id, false);
    field(&mut mentions, "title", LogicalType::String, false);
    field(&mut mentions, "local_title", LogicalType::String, false);
    field(&mut mentions, "entry", LogicalType::Id, false);
    field(
        &mut mentions,
        "contributors",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    mentions
        .register_field(FieldDefinition {
            path: vec!["items".into(), "entry".into()],
            kind: LogicalType::Id,
            required: false,
            nullable: true,
            indexed: false,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    relation(&mut mentions, "entry", "entries", false, "mentions");
    relation(
        &mut mentions,
        "contributors",
        "entries",
        true,
        "contributions",
    );
    mentions
        .register_relation(RelationDefinition {
            path: vec!["items".into(), "entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::Many,
            reverse_alias: Some("item_mentions".into()),
        })
        .unwrap();
    (
        root,
        vec![
            definition("entries", entries),
            definition("mentions", mentions),
        ],
    )
}

fn query(
    collection: &str,
    filter: Option<&str>,
    fts: Option<&str>,
    field: Option<&str>,
    regex: bool,
) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "relation".into(),
        collection: collection.into(),
        fts: fts.map(str::to_owned),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: if regex {
            FtsMode::Regex(Default::default())
        } else {
            FtsMode::Literal
        },
        filter: filter.map(str::to_owned),
        fts_fields: field
            .into_iter()
            .flat_map(|fields| fields.split(',').map(str::to_owned))
            .collect(),
        sort: vec![],
        limit: 50,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    })
    .unwrap()
}

fn ids(repo: &Repository, request: Query) -> Vec<String> {
    let optimized = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(optimized.count, reference.count);
    optimized
        .hits
        .iter()
        .map(|hit| hit.record_id.clone())
        .collect()
}

#[test]
fn related_all_results_stream_in_identity_order_under_small_budget() {
    let (root, definitions) = fixture();
    for index in 0..110 {
        std::fs::write(
            root.join(format!("mentions/bulk-{index:03}.json")),
            format!(
                r#"{{"id":"bulk-{index:03}","entry":"A","local_title":"Hymn","title":"bulk"}}"#
            ),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for mut request in [
        query(
            "mentions",
            Some("local_title = entry.title"),
            None,
            None,
            false,
        ),
        query("mentions", None, Some("Hymn"), Some("entry.title"), false),
    ] {
        request.options.limit = 200;
        let reference = repo.search_reference(&request).unwrap();
        let mut rows = Vec::new();
        let mut first_seen = None;
        let outcome = repo
            .search_stream_scoped(
                &request,
                ResultScope::All { offset: 0 },
                StreamOptions {
                    batch_rows: 3,
                    max_event_bytes: 2048,
                    max_buffered_bytes: 4096,
                    ..Default::default()
                },
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows {
                        start,
                        rows: batch,
                        progress,
                        ..
                    } = event
                    {
                        assert_eq!(start, rows.len());
                        first_seen.get_or_insert(progress.observed_matches);
                        rows.extend(batch.iter().cloned());
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("expected complete")
        };
        assert_eq!(summary.count, reference.count);
        assert_eq!(rows, reference.hits);
        assert!(rows.len() > 50);
        assert!(first_seen.unwrap() < rows.len() as u64);
        assert!(summary.progress.peak_buffered_bytes <= 4096);
    }
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn related_reducers_preserve_source_reference_occurrences() {
    let (root, definitions) = fixture();
    std::fs::write(root.join("entries/a.json"), r#"{"id":"A","title":"Alpha"}"#).unwrap();
    std::fs::write(root.join("entries/b.json"), r#"{"id":"B","title":"Zulu"}"#).unwrap();
    std::fs::write(
        root.join("entries/c.json"),
        r#"{"id":"C","title":"Middle"}"#,
    )
    .unwrap();
    for (name, values) in [
        ("order_bab", "[\"B\",\"A\",\"B\"]"),
        ("order_a", "[\"A\"]"),
        ("order_b", "[\"B\"]"),
        ("order_m", "[\"C\"]"),
    ] {
        std::fs::write(
            root.join(format!("mentions/{name}.json")),
            format!(r#"{{"id":"{name}","contributors":{values}}}"#),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (reducer, expected) in [
        ("FIRST", vec!["order_a", "order_m", "order_b", "order_bab"]),
        ("LAST", vec!["order_a", "order_m", "order_b", "order_bab"]),
        ("MIN", vec!["order_a", "order_bab", "order_m", "order_b"]),
        ("MAX", vec!["order_a", "order_m", "order_b", "order_bab"]),
    ] {
        let expression = format!(
            "(id = 'order_a' || id = 'order_b' || id = 'order_m' || id = 'order_bab') SORT BY {reducer}(contributors.title)"
        );
        let query = query("mentions", Some(&expression), None, None, false);
        let result = repo.search(&query).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{reducer}"
        );
        assert_eq!(
            result.hits,
            repo.search_reference(&query).unwrap().hits,
            "{reducer}"
        );
    }
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn related_sort_does_not_reuse_an_incomplete_filter_target_projection() {
    let (root, definitions) = fixture();
    std::fs::write(
        root.join("entries/a.json"),
        r#"{"id":"A","title":"Hymn","alt_title":"Zulu"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("entries/b.json"),
        r#"{"id":"B","title":"Festival","alt_title":"Alpha"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m8.json"),
        r#"{"id":"m8","entry":"B","local_title":"Festival"}"#,
    )
    .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let sorted = query(
        "mentions",
        Some("entry.title = local_title SORT BY entry.alt_title ASC"),
        None,
        None,
        false,
    );
    let result = repo.search(&sorted).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m8", "m1"]
    );
    assert_eq!(result.hits, repo.search_reference(&sorted).unwrap().hits);
    assert_eq!(result.counters.sort_target_reads, 2);

    let shared = query(
        "mentions",
        Some("entry.title = local_title SORT BY entry.title ASC"),
        None,
        None,
        false,
    );
    let result = repo.search(&shared).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m8", "m1"]
    );
    assert_eq!(result.counters.sort_target_reads, 0);
    assert!(result.counters.sort_target_reuse >= 2);
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn related_sort_stream_reports_target_budget_as_resource_error() {
    let (root, definitions) = fixture();
    std::fs::write(
        root.join("entries/a.json"),
        format!(
            r#"{{"id":"A","title":"Hymn","alt_title":"{}"}}"#,
            "Z".repeat(3000)
        ),
    )
    .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let request = query(
        "mentions",
        Some("id = 'm1' SORT BY entry.alt_title"),
        None,
        None,
        false,
    );
    let error = repo
        .search_stream(
            &request,
            StreamOptions {
                max_buffered_bytes: 4096,
                max_event_bytes: 256,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |_| Ok(crate::streaming::StreamControl::Continue),
        )
        .unwrap_err();
    assert!(matches!(error, StreamError::Resource { .. }), "{error:?}");
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn finite_relation_paths_have_independent_reference_answers() {
    let (root, definitions) = fixture();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (filter, expected) in [
        ("entry.title = 'Hymn'", vec!["m1"]),
        ("entry.title:lower = 'hymn'", vec!["m1"]),
        ("entry.title ?!= 'Hymn'", vec!["m2"]),
        ("contributors.title = 'Hymn'", vec!["m7"]),
        ("contributors.title ?= 'Hymn'", vec!["m6"]),
        ("contributors[0].title = 'Hymn'", vec!["m6"]),
        ("contributors[1].title = 'Hymn'", vec![]),
        ("items[0].entry.title = 'Hymn'", vec!["m1"]),
        ("items[1].entry.title = 'Festival'", vec!["m1"]),
        ("local_title = entry.title", vec!["m1"]),
        ("entry.title = local_title", vec!["m1"]),
        ("entry.title = entry.alt_title", vec!["m1"]),
        ("contributors.title ?= contributors.alt_title", vec!["m6"]),
        ("entry.related_entry.title = 'Festival'", vec!["m1"]),
        (
            "entry.related_entry.related_entry.title = 'Hymn'",
            vec!["m1"],
        ),
    ] {
        assert_eq!(
            ids(&repo, query("mentions", Some(filter), None, None, false)),
            expected,
            "{filter}"
        );
    }
    assert_eq!(
        ids(
            &repo,
            query(
                "entries",
                Some("@backlinks.mentions.title ?~ 'Festival'"),
                None,
                None,
                false
            )
        ),
        ["A"],
    );
    assert_eq!(
        ids(
            &repo,
            query("mentions", None, Some("Hymn"), Some("entry.title"), false)
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query("mentions", None, Some("^Hymn$"), Some("entry.title"), true)
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query(
                "mentions",
                Some("title = 'Festival'"),
                Some("Hymn"),
                Some("entry.title"),
                false
            )
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query(
                "entries",
                None,
                Some("Festival"),
                Some("@backlinks.mentions.title"),
                false
            )
        ),
        ["A"],
    );
    assert!(
        repo.search(&query(
            "entries",
            Some("@backlinks.unknown.title = 'x'"),
            None,
            None,
            false
        ))
        .is_err()
    );
    let mut sort = query("mentions", None, None, None, false);
    sort.options.sort.push(crate::execution_options::SortField {
        field: "entry.title".into(),
        descending: false,
        expression: None,
    });
    let optimized = repo.search(&sort).unwrap();
    let reference = repo.search_reference(&sort).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(
        optimized
            .hits
            .iter()
            .take(7)
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1", "m4", "m3", "m5", "m6", "m7"]
    );
}

#[test]
fn unavailable_targets_survive_negation_and_source_records_are_counted_once() {
    let (root, definitions) = fixture();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (filter, expected) in [
        (
            "!(entry.title = 'Hymn')",
            vec!["m2", "m4", "m5", "m6", "m7"],
        ),
        ("!(entry.title = 'Hymn') && @record.id = 'm3'", vec![]),
        ("@file.status = 1 && !(entry.title = 'Hymn')", vec![]),
        (
            "!(contributors.title ?= 'Hymn')",
            vec!["m1", "m2", "m3", "m4", "m5", "m7"],
        ),
        ("contributors.title ?!= 'Hymn'", vec!["m6"]),
    ] {
        assert_eq!(
            ids(&repo, query("mentions", Some(filter), None, None, false)),
            expected,
            "{filter}"
        );
    }
    let fallback_or = query(
        "mentions",
        Some("@file.status = 1 || entry.title = 'Hymn'"),
        None,
        None,
        false,
    );
    let optimized = repo.search(&fallback_or).unwrap();
    let reference = repo.search_reference(&fallback_or).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(optimized.count, Some(2));
    assert!(optimized.hits.iter().any(|hit| hit.record_id == "m1"));
    assert!(
        optimized
            .hits
            .iter()
            .any(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback)
    );
    let selected = query(
        "mentions",
        Some("contributors.title ?= 'Hymn'"),
        None,
        None,
        false,
    );
    let found = repo.search(&selected).unwrap();
    assert_eq!(found.count, Some(1));
    assert_eq!(found.hits[0].record_id, "m6");
    assert!(
        found.hits[0]
            .matched_fields
            .contains(&"contributors.title".into())
    );
    assert!(found.counters.relation_reverse_postings_visited >= 1);
    assert!(found.counters.relation_target_candidates >= 1);
    assert_eq!(found.counters.relation_source_reads, 0);
    let reference = repo.search_reference(&selected).unwrap();
    assert!(reference.counters.relation_source_reads > 0);
}

#[test]
fn relation_answers_and_evidence_survive_cache_and_snapshot_restore() {
    let (root, definitions) = fixture();
    let options = RepositoryOptions {
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions.clone(),
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    repo.configure_verified_cache(1 << 20).unwrap();
    for filter in [
        "entry.title = 'Hymn'",
        "entry.title = 'Festival'",
        "!(entry.title = 'Hymn')",
        "entry.title = 'Hymn'",
    ] {
        let mut request = query("mentions", Some(filter), None, None, false);
        request.options.limit = 2;
        request.options.offset = 1;
        for _ in 0..2 {
            let actual = repo.search(&request).unwrap();
            let expected = repo.search_reference(&request).unwrap();
            assert_eq!(actual.count, expected.count, "{filter}");
            assert_eq!(actual.hits, expected.hits, "{filter}");
        }
    }
    for (mode, expected) in [
        (CaseSensitivity::Sensitive, 0),
        (CaseSensitivity::Insensitive, 1),
        (CaseSensitivity::Sensitive, 0),
    ] {
        let mut request = query("mentions", None, Some("HYMN"), Some("entry.title"), false);
        request.fts.as_mut().unwrap().case_sensitivity = mode;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(actual.count, Some(expected));
        assert_eq!(actual.hits, reference.hits);
    }
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for filter in ["entry.title = 'Hymn'", "contributors.title ?= 'Hymn'"] {
        let request = query("mentions", Some(filter), None, None, false);
        assert_eq!(
            repo.search(&request).unwrap().hits,
            restored.search(&request).unwrap().hits
        );
        assert_eq!(
            restored.search(&request).unwrap().hits,
            restored.search_reference(&request).unwrap().hits
        );
    }
}

#[test]
fn membership_only_matches_complete_evidence_for_local_relation_and_fts_routes() {
    let (root, definitions) = fixture();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let borrowed_targets_before = super::borrowed_relation_targets();
    for request in [
        query(
            "mentions",
            Some("entry.title = 'Hymn' || local_title = 'Hymn'"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            Some("items[0].entry.title = 'Hymn'"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            Some("!(entry.title = 'Hymn')"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            None,
            Some("Hymn"),
            Some("local_title,entry.title"),
            false,
        ),
        query("mentions", None, Some("^Hymn$"), Some("entry.title"), true),
        query("mentions", None, Some("Hymn"), None, false),
    ] {
        let with_evidence = repo.search(&request).unwrap();
        assert_eq!(
            with_evidence.hits,
            repo.search_reference(&request).unwrap().hits
        );
        let mut membership_request = request.clone();
        membership_request.options.evidence = false;
        let actual = repo.search(&membership_request).unwrap();
        let reference = repo.search_reference(&membership_request).unwrap();
        assert_eq!(actual.count, with_evidence.count);
        assert_eq!(actual.hits, reference.hits);
        let mut expected = with_evidence.hits;
        for hit in &mut expected {
            hit.matched_fields.clear();
        }
        assert_eq!(actual.hits, expected);
    }
    assert!(super::borrowed_relation_targets() > borrowed_targets_before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mixed_local_and_relation_fts_borrows_source_value_text() {
    let (root, definitions) = fixture();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            query_workers: 1,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let request = query(
        "mentions",
        None,
        Some("Hymn"),
        Some("local_title,entry.title"),
        false,
    );
    for mode in [ReturnMode::Locator, ReturnMode::Logical] {
        for evidence in [false, true] {
            let mut request = request.clone();
            request.options.return_mode = mode.clone();
            request.options.evidence = evidence;
            let copied_before = crate::source::text::owned_collector_copied_bytes();
            let actual = repo.search(&request).unwrap();
            assert_eq!(
                crate::source::text::owned_collector_copied_bytes(),
                copied_before
            );
            assert_eq!(actual.hits, repo.search_reference(&request).unwrap().hits);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_fts_rejects_disagreeing_public_field_and_query_path() {
    let (root, definitions) = fixture();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (selected, hidden_path) in [("entry.title", "entry.alt_title"), ("title", "local_title")] {
        let mut request = query("mentions", None, Some("Hymn"), Some(selected), false);
        request.fts.as_mut().unwrap().fields[0].query_path = Some(hidden_path.into());
        for result in [repo.search(&request), repo.search_reference(&request)] {
            assert!(
                result.unwrap_err().contains("disagrees with its field"),
                "{selected} and {hidden_path}"
            );
        }
    }
}
