use crate::records::RecordIdSlot;
use std::path::PathBuf;
use std::sync::Arc;

use crate::model::LogicalType;
use crate::query::{CaseSensitivity, FtsMode, Query};
use crate::query_execution::relations::view::MergedRelationView;
use crate::query_execution::relations::work::RelationWork;
use crate::repository::Repository;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::relations::{RelationCardinality, RelationDefinition};
use crate::schema::{FieldDefinition, Schema};

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
                definition: collection.required_file_definition(),
                source: &collection.required_file_reader(),
            }],
            repo.options().bigram_alphabet,
            repo.options().inference,
            repo.collections().len(),
        )
        .unwrap();
        let mut admission = SegmentAdmission::new(
            CollectionId(collection_index as u32),
            &collection.required_file_reader(),
        );
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
            crate::source::test_support::fixture_registry(),
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
    std::fs::remove_file(root.join("entries/a.row")).unwrap();

    std::fs::write(
        root.join("entries/new-a.row"),
        r#"("id"="A","title"="New Hymn")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/new-m.row"),
        r#"("id"="m8","entry"="A","title"="New source")"#,
    )
    .unwrap();
    let next_file = repo.files().files.len() as u32;
    let target = stage(&repo, &root, 0, FileId(next_file), "entries/new-a.row");
    let source = stage(&repo, &root, 1, FileId(next_file + 1), "mentions/new-m.row");
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
            observation: None,
        })]),
        Arc::new(vec![Arc::new(PublishedSegment {
            id: crate::index::segment::SegmentId::new(2),
            active: ActiveRecords::all(source.records.len()).unwrap(),
            index: Arc::new(source),
            observation: None,
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
    let outgoing = crate::repository::relations::relation_outgoing_page_segmented(
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
    let nested = crate::repository::relations::relation_outgoing_page_segmented(
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
    let null = crate::repository::relations::relation_outgoing_page_segmented(
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
        crate::repository::relations::RelationPresence::Present
    );
    assert!(null.contains_null);
    assert_eq!(null.total_occurrences, 0);
    let backlinks = crate::repository::relations::relation_backlink_page_segmented(
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
    let backlink_tail = crate::repository::relations::relation_backlink_page_segmented(
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
    let graph = crate::repository::relations::collection_graph_segmented(
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
    let source_retired_backlinks = crate::repository::relations::relation_backlink_page_segmented(
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
    let source_retired_graph = crate::repository::relations::collection_graph_segmented(
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
    sorted.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
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
    first.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::query::options::SortReducer::First),
        },
        false,
    )];
    let first_order = find(first);
    let first_at = |id: &str| first_order.iter().position(|found| found == id).unwrap();
    assert!(first_at("m2") < first_at("m1"));
    let mut last = query("mentions", None, None, None, false);
    last.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::query::options::SortReducer::Last),
        },
        false,
    )];
    let last_order = find(last);
    let last_at = |id: &str| last_order.iter().position(|found| found == id).unwrap();
    assert!(last_at("m1") < last_at("m2"));
    let mut backlink_sort = query("entries", None, None, None, false);
    backlink_sort.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
            path: "@backlinks.mentions.title".into(),
            binding: None,
            reducer: Some(crate::query::options::SortReducer::Min),
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
    let unresolved = crate::repository::relations::relation_outgoing_page_segmented(
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
        crate::repository::relations::relation_backlink_page_segmented(
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
    let retired_graph = crate::repository::relations::collection_graph_segmented(
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
    std::fs::remove_file(root.join("entries/new-a.row")).unwrap();
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
    unavailable_sort.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
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
            crate::source::test_support::fixture_registry(),
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
            Some(crate::query::options::SortReducer::First),
        ),
        (
            "mentions",
            "items.entry.title",
            Some(crate::query::options::SortReducer::Last),
        ),
        (
            "mentions",
            "contributors.title",
            Some(crate::query::options::SortReducer::Min),
        ),
        (
            "entries",
            "@backlinks.mentions.title",
            Some(crate::query::options::SortReducer::Min),
        ),
    ] {
        let mut request = query(collection, None, None, None, false);
        request.options.sort = vec![crate::query::options::SortField::computed(
            crate::query::options::SortExpression {
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
use crate::query::options::ReturnMode;
use crate::query::test_support::{QueryFixture, query_from_fixture};
use crate::query_execution::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamError, StreamEvent, StreamOptions,
    StreamOutcome,
};

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
        format: crate::source::test_support::document_format(),
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
        crate::source::test_support::fixture_registry(),
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
    let target = root.join("entries/a.row");
    let metadata = std::fs::metadata(&target).unwrap();
    std::fs::rename(&target, root.join("entries/hidden-a.row")).unwrap();
    std::fs::write(&target, vec![b'x'; metadata.len() as usize]).unwrap();
    std::fs::File::open(&target)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    let replaced_revision =
        SourceRevision::from_metadata(&std::fs::metadata(&target).unwrap()).unwrap();
    let original_revision = SourceRevision::from_metadata(&metadata).unwrap();
    assert_eq!(replaced_revision.size, original_revision.size);
    assert_eq!(replaced_revision.modified_ns, original_revision.modified_ns);
    assert_ne!(replaced_revision.object_id, original_revision.object_id);
    let mut bytes = query(
        "mentions",
        Some("entry.title = 'Hymn' && @record.id = 'm1'"),
        None,
        None,
        false,
    );
    bytes.options.return_mode = ReturnMode::Bytes;
    // The pinned source revision must reject externally replaced bytes.
    let stale = view.search(&bytes).unwrap();
    assert_eq!(stale.count, None);
    assert!(stale.counters.source_unavailable_events > 0);
    assert!(stale.hits.is_empty());
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
        crate::source::test_support::fixture_registry(),
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
    request.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
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
fn scalar_relation_wildcards_do_not_reuse_selector_free_sort_keys() {
    use crate::model::{CollectionId, FieldPath};

    let (root, mut definitions) = fixture();
    definitions[0]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..RepositoryOptions::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();

    let wildcard = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry[*].title ASC"),
        None,
        None,
        false,
    );
    let optimized = view.search(&wildcard).unwrap();
    let reference = view.search_reference(&wildcard).unwrap();
    assert_eq!(optimized.count, Some(2));
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    let mut first = wildcard.clone();
    first.options.limit = 1;
    for result in [
        view.search(&first).unwrap(),
        view.search_reference(&first).unwrap(),
    ] {
        assert_eq!(result.count, Some(2));
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["m1"]
        );
    }

    let selector_free = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry.title ASC"),
        None,
        None,
        false,
    );
    let optimized = view.search(&selector_free).unwrap();
    let reference = view.search_reference(&selector_free).unwrap();
    assert_eq!(optimized.count, Some(2));
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert_eq!(optimized.counters.sort_target_reads, 0);
    let mut first = selector_free.clone();
    first.options.limit = 1;
    for result in [
        view.search(&first).unwrap(),
        view.search_reference(&first).unwrap(),
    ] {
        assert_eq!(result.count, Some(2));
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["m2"]
        );
    }

    for request in [
        query(
            "mentions",
            Some("entry[*].title = 'Hymn'"),
            None,
            None,
            false,
        ),
        query(
            "mentions",
            None,
            Some("Hymn"),
            Some("entry[*].title"),
            false,
        ),
    ] {
        let optimized = view.search(&request).unwrap();
        let reference = view.search_reference(&request).unwrap();
        assert_eq!(optimized.count, reference.count, "{request:?}");
        assert_eq!(optimized.hits, reference.hits, "{request:?}");
    }

    let projected = query(
        "mentions",
        Some("entry.title = local_title SORT BY entry[*].title ASC"),
        None,
        None,
        false,
    );
    let projected_result = view.search(&projected).unwrap();
    assert_eq!(
        projected_result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1"]
    );
    assert_eq!(
        projected_result.hits,
        view.search_reference(&projected).unwrap().hits
    );

    let bound = crate::query::sort_binding::BoundSort::bind(view.repository(), &projected).unwrap();
    let crate::query::sort_binding::BoundSortSource::Computed(computed) =
        &bound.terms[1].as_ref().unwrap()[0].source
    else {
        panic!("related sort must use a computed source evaluator");
    };
    let prepared = crate::query::prepared::prepare(view.repository(), &projected).unwrap();
    let filter = prepared.collections[0].filter().unwrap();
    let merged = MergedRelationView::new(
        view.repository(),
        &view.0.active,
        &view.0.segments,
        &view.0.catalog,
    )
    .unwrap();
    let address = merged.resolve_id(CollectionId(1), "m1").unwrap().unwrap();
    let value = merged.reference_value(address, None).unwrap().unwrap();
    let resolved = merged.record(address).unwrap().unwrap();
    let ctx = crate::query::semantics::record::RecordContext {
        value: &value,
        source_bytes: None,
        record: &resolved.record,
        range: (resolved.record).record_range(),
        file: Some(resolved.file),
        collection_name: &resolved.collection.definition.name,
        repository_root: Some(merged.repository_root()),
    };
    let mut evidence = std::collections::HashSet::new();
    let mut work = RelationWork::default();
    assert_eq!(
        super::eval_filter_truth_merged(
            &merged,
            filter,
            address,
            &ctx,
            &mut evidence,
            &mut work,
            false,
        )
        .unwrap(),
        Some(true)
    );
    assert!(
        crate::query_results::ranking::cached_filter_target_sort_key(
            &merged,
            address,
            computed,
            Some(filter),
        )
        .unwrap()
        .is_none()
    );
    assert!(
        crate::query_results::ranking::direct_related_target_sort_key(
            view.repository(),
            &merged,
            &view.0.segments,
            address,
            computed,
        )
        .unwrap()
        .is_none()
    );

    let array_control = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY MIN(items.entry[*].title)"),
        None,
        None,
        false,
    );
    let optimized = view.search(&array_control).unwrap();
    let reference = view.search_reference(&array_control).unwrap();
    assert_eq!(optimized.count, Some(2));
    assert_eq!(optimized.hits, reference.hits);
    assert!(optimized.counters.sort_target_reads > 0);

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn backlink_sort_does_not_use_a_forward_relation_at_the_same_ordinal() {
    use crate::model::CollectionId;
    use crate::model::FieldPath;

    let (root, mut definitions) = fixture();
    definitions[0]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Zulu")"#).unwrap();
    std::fs::write(
        root.join("entries/b.row"),
        r#"("id"="B","title"="Bravo","related_entry"="A")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m1.row"),
        r#"("id"="m1","entry"="A","title"="Zulu")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.row"),
        r#"("id"="m2","entry"="B","title"="Bravo")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m3.row"),
        r#"("id"="m3","entry"="A","title"="Alpha")"#,
    )
    .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();

    let sort_only = query(
        "entries",
        Some("(id = 'A' || id = 'B') SORT BY @backlinks.mentions[0].title ASC"),
        None,
        None,
        false,
    );
    let optimized = repo.search(&sort_only).unwrap();
    let reference = repo.search_reference(&sort_only).unwrap();
    assert_eq!(
        optimized.count,
        Some(2),
        "optimized {:?}, reference {:?}",
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["B", "A"]
    );
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(optimized.counters.sort_target_reads, 2);
    assert_eq!(optimized.counters.relation_edge_visits, 3);
    assert_eq!(optimized.counters.relation_target_reads, 0);

    let mut limited = sort_only.clone();
    limited.options.limit = 1;
    assert_eq!(
        repo.search(&limited)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );

    let projected = query(
        "entries",
        Some(
            "title = @backlinks.mentions.title && (id = 'A' || id = 'B') \
             SORT BY @backlinks.mentions[0].title ASC",
        ),
        None,
        None,
        false,
    );
    let optimized = repo.search(&projected).unwrap();
    let reference = repo.search_reference(&projected).unwrap();
    assert_eq!(optimized.count, Some(1));
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
    assert_eq!(optimized.hits, reference.hits);

    let bound = crate::query::sort_binding::BoundSort::bind(&repo, &projected).unwrap();
    let crate::query::sort_binding::BoundSortSource::Computed(computed) =
        &bound.terms[0].as_ref().unwrap()[0].source
    else {
        panic!("backlink sort must use a computed source evaluator");
    };
    let prepared = crate::query::prepared::prepare(&repo, &projected).unwrap();
    let filter = prepared.collections[0].filter().unwrap();
    let view = MergedRelationView::new(&repo, &repo.active, &repo.segments, &repo.catalog).unwrap();
    let address = view.resolve_id(CollectionId(0), "B").unwrap().unwrap();
    assert!(
        crate::query_results::ranking::cached_filter_target_sort_key(
            &view,
            address,
            computed,
            Some(filter),
        )
        .unwrap()
        .is_none(),
        "the cached-filter shortcut must decline a backlink hop"
    );
    assert!(
        crate::query_results::ranking::direct_related_target_sort_key(
            &repo,
            &view,
            &repo.segments,
            address,
            computed,
        )
        .unwrap()
        .is_none(),
        "the direct shortcut must decline a backlink hop"
    );

    drop(repo);
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
            root.join(format!("mentions/extra-{number:03}.row")),
            format!(
                r#"("id"="extra-{number:03}","entry"="A","title"="{:03}")"#,
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
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    for descending in [false, true] {
        for offset in [0, 17, 70] {
            for count in [false, true] {
                let mut request =
                    query("mentions", Some("entry.title = 'Hymn'"), None, None, false);
                request.options.sort = vec![crate::query::options::SortField::simple(
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
    use crate::model::FieldPath;
    use crate::query::options::{SortExpression, SortField};
    use std::collections::HashMap;

    let (root, mut definitions) = fixture();
    definitions[1]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Target")"#).unwrap();
    std::fs::write(root.join("entries/b.row"), r#"("id"="B","title"="Other")"#).unwrap();
    std::fs::write(
        root.join("mentions/m1.row"),
        r#"("id"="m1","entry"="A","title"="tie","local_title"="residual alpha")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.row"),
        r#"("id"="m2","entry"="A","title"="retired old","local_title"="not selected")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m4.row"),
        r#"("id"="m4","entry"="B","title"="zzz","local_title"="residual needle")"#,
    )
    .unwrap();
    std::fs::remove_file(root.join("mentions/m3.row")).unwrap();

    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let before = index.view().unwrap();
    let m2 = before
        .file_handle_for_path(std::path::Path::new("mentions/m2.row"))
        .unwrap();
    std::fs::write(
        root.join("mentions/m2.row"),
        r#"("id"="m2","entry"="A","title"="tie","local_title"="residual beta")"#,
    )
    .unwrap();
    index
        .update_file(m2, &root.join("mentions/m2.row"))
        .unwrap();
    std::fs::write(
        root.join("mentions/m3.row"),
        r#"("id"="m3","entry"="A","title"="tie","local_title"="residual gamma")"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m3.row")).unwrap();

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
    use crate::schema::relations::RelationId;

    let (root, definitions) = fixture();
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Hymn")"#).unwrap();
    std::fs::write(root.join("entries/b.row"), r#"("id"="B","title"="Other")"#).unwrap();
    std::fs::write(
        root.join("mentions/m1.row"),
        r#"("id"="m1","entry"="B","title"="base")"#,
    )
    .unwrap();
    std::fs::remove_file(root.join("mentions/m2.row")).unwrap();
    std::fs::remove_file(root.join("mentions/m3.row")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    std::fs::write(
        root.join("mentions/m2.row"),
        r#"("id"="m2","entry"="A","title"="segment")"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m2.row")).unwrap();
    std::fs::write(
        root.join("mentions/m3.row"),
        r#"("id"="m3","entry"="A","title"="segment")"#,
    )
    .unwrap();
    index.add_file(&root.join("mentions/m3.row")).unwrap();

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
                observation: None,
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
        MergedRelationView::new(repo, &pinned.0.active, &perturbed, &pinned.0.catalog).unwrap();
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
fn segmented_reference_first_last_ignores_unavailable_contribution() {
    use crate::index::segment::PublishedSegment;
    use crate::model::{CollectionId, Value};
    use crate::query_results::Payload;

    let (root, definitions) = fixture();
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Alpha")"#).unwrap();
    std::fs::write(root.join("entries/b.row"), r#"("id"="B","title"="Zulu")"#).unwrap();
    std::fs::remove_file(root.join("mentions/m2.row")).unwrap();
    std::fs::remove_file(root.join("mentions/m3.row")).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    for (name, items) in [
        (
            "m2",
            r#"[("entry"=null),("entry"="B"),("entry"="A"),("entry"="A")]"#,
        ),
        ("m3", r#"[("entry"="A"),("entry"="B")]"#),
    ] {
        std::fs::write(
            root.join(format!("mentions/{name}.row")),
            format!(r#"("id"="{name}","items"={items})"#),
        )
        .unwrap();
        index
            .add_file(&root.join(format!("mentions/{name}.row")))
            .unwrap();
    }

    let pinned = index.view().unwrap();
    let repo = pinned.repository();
    let relation_index = repo.collections[1]
        .relations
        .iter()
        .position(|relation| relation.declaration.path == ["items", "entry"])
        .expect("items.entry relation");
    let mut perturbed = pinned.0.segments.clone();
    perturbed[1] = Arc::new(
        pinned.0.segments[1]
            .iter()
            .map(|published| {
                let mut segment = (*published.index).clone();
                if let Some(slot) = segment.id_lookup.get("m2").copied() {
                    segment.relations[relation_index].mark_unavailable_for_test(slot);
                }
                Arc::new(PublishedSegment {
                    id: published.id,
                    index: Arc::new(segment),
                    active: published.active.clone(),
                    observation: None,
                })
            })
            .collect::<Vec<_>>(),
    );
    let diagnostic_view =
        MergedRelationView::new(repo, &pinned.0.active, &perturbed, &pinned.0.catalog).unwrap();
    let m2 = diagnostic_view
        .resolve_id(CollectionId(1), "m2")
        .unwrap()
        .unwrap();
    assert!(diagnostic_view.forward_edges(m2, relation_index).unwrap().4);
    drop(diagnostic_view);

    for (reducer, expected) in [("FIRST", ["m3", "m2"]), ("LAST", ["m2", "m3"])] {
        let mut request = query(
            "mentions",
            Some(&format!(
                "(id = 'm2' || id = 'm3') SORT BY {reducer}(items.entry.title)"
            )),
            None,
            None,
            false,
        );
        request.options.return_mode = ReturnMode::Logical;
        request.options.limit = 10;
        let reference = crate::reference::relations::search_segmented(
            repo,
            &request,
            &pinned.0.active,
            &perturbed,
            &pinned.0.catalog,
            pinned.revision(),
        )
        .unwrap();
        assert_eq!(reference.count, Some(2), "{reducer}");
        assert_eq!(
            reference
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{reducer}"
        );
        assert!(reference.hits.iter().all(|hit| matches!(
            &hit.payload,
            Some(Payload::Logical(Value::Object(fields)))
                if fields.get("id") == Some(&Value::Id(hit.record_id.clone()))
        )));
    }

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn base_reference_sort_ignores_perturbed_stored_and_relation_contributions_in_adopted_path() {
    use crate::index::sorting::SortKey;
    use crate::model::{FieldPath, RecordSlot};

    let (root, mut definitions) = fixture();
    definitions[1]
        .sort_fields
        .push(FieldPath::parse("title").unwrap());
    let mut repo = base_repository(&root, definitions);
    let m1 = repo.collections[1].id_lookup["m1"].0 as usize;
    let m2 = repo.collections[1].id_lookup["m2"].0 as usize;
    let column = repo.collections[1]
        .sorting
        .columns
        .get_mut(&FieldPath::parse("title").unwrap())
        .expect("resident title order");
    column.keys.swap(m1, m2);
    assert_ne!(column.keys[m1], SortKey::Missing);

    let stored = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY title"),
        None,
        None,
        false,
    );
    let reference = repo.search_reference(&stored).unwrap();
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
    assert!(
        reference
            .hits
            .iter()
            .all(|hit| hit.matched_fields == ["id"])
    );
    let relation_index = repo.collections[1]
        .relations
        .iter()
        .position(|relation| relation.declaration.path == ["entry"])
        .expect("entry relation");
    repo.collections[1].relations[relation_index].mark_unavailable_for_test(RecordSlot(m1 as u32));
    repo.collections[1].relations[relation_index].mark_unavailable_for_test(RecordSlot(m2 as u32));
    let related = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry.title"),
        None,
        None,
        false,
    );
    let reference = repo.search_reference(&related).unwrap();
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    std::fs::remove_file(root.join("entries/a.row")).unwrap();
    let unavailable = repo.search_reference(&related).unwrap();
    assert_eq!(unavailable.count, None);
    assert!(unavailable.counters.source_unavailable_events > 0);
    assert!(!unavailable.counters.source_diagnostics.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopted_base_view_reference_search_uses_pinned_publication() {
    use crate::query::options::ReturnMode;
    use crate::query_results::Payload;

    let (root, definitions) = fixture();
    let repo = base_repository(&root, definitions);
    let mut request = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry.title"),
        None,
        None,
        false,
    );
    request.options.limit = 1;
    let expected = repo.search_reference(&request).unwrap();
    assert_eq!(expected.count, Some(2));
    assert_eq!(expected.hits.len(), 1);
    assert_eq!(expected.hits[0].record_id, "m2");
    assert_eq!(expected.hits[0].matched_fields, ["id"]);
    assert_eq!(
        expected.hits[0].file.as_ref().unwrap().path,
        std::path::Path::new("mentions/m2.row")
    );

    let index = crate::lifecycle::Index::from_repository(repo).unwrap();
    let view = index.view().unwrap();
    assert!(view.0.active.iter().all(|active| active.len() > 0));
    assert_eq!(view.0.segments.len(), view.repository().collections.len());
    assert!(view.0.segments.iter().all(|segments| segments.is_empty()));

    let locator = view.search_reference(&request).unwrap();
    assert_eq!(locator.count, Some(2));
    assert_eq!(locator.hits.len(), 1);
    assert_eq!(locator.hits[0].record_id, "m2");
    assert_eq!(locator.hits[0].matched_fields, ["id"]);
    assert_eq!(locator.hits[0].generation, view.revision().0);
    assert_eq!(
        locator.hits[0].file.as_ref().unwrap().path,
        std::path::Path::new("mentions/m2.row")
    );
    assert!(locator.hits[0].payload.is_none());

    let mut logical_request = request.clone();
    logical_request.options.return_mode = ReturnMode::Logical;
    let logical = view.search_reference(&logical_request).unwrap();
    assert!(matches!(
        &logical.hits[0].payload,
        Some(Payload::Logical(crate::model::Value::Object(fields)))
            if fields.get("id") == Some(&crate::model::Value::Id("m2".into()))
    ));

    let mut bytes_request = request.clone();
    bytes_request.options.return_mode = ReturnMode::Bytes;
    let bytes = view.search_reference(&bytes_request).unwrap();
    assert_eq!(
        bytes.hits[0].payload,
        Some(Payload::Bytes(
            std::fs::read(root.join("mentions/m2.row")).unwrap()
        ))
    );

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopted_base_view_reference_search_keeps_retired_rows_on_the_old_pin() {
    let (root, definitions) = fixture();
    let repo = base_repository(&root, definitions);
    let index = crate::lifecycle::Index::from_repository(repo).unwrap();
    let pinned = index.view().unwrap();
    let request = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry.title"),
        None,
        None,
        false,
    );
    let old = pinned.search_reference(&request).unwrap();
    assert_eq!(old.count, Some(2));
    assert_eq!(
        old.hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert!(
        old.hits
            .iter()
            .all(|hit| hit.generation == pinned.revision().0 && hit.matched_fields == ["id"])
    );

    let retired = pinned
        .file_handle_for_path(std::path::Path::new("mentions/m2.row"))
        .unwrap();
    let deletion = index.delete_file(retired).unwrap();
    assert_eq!(deletion.retired_records, 1);
    assert!(root.join("mentions/m2.row").exists());

    let still_old = pinned.search_reference(&request).unwrap();
    assert_eq!(still_old.count, Some(2));
    assert_eq!(
        still_old
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert!(
        still_old
            .hits
            .iter()
            .all(|hit| hit.generation == pinned.revision().0)
    );

    let current = index.view().unwrap();
    assert_eq!(current.revision(), deletion.revision);
    let after = current.search_reference(&request).unwrap();
    assert_eq!(after.count, Some(1));
    assert_eq!(after.hits.len(), 1);
    assert_eq!(after.hits[0].record_id, "m1");
    assert_eq!(after.hits[0].generation, current.revision().0);

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopted_base_view_reference_search_uses_each_pins_catalogue() {
    use crate::query::options::ReturnMode;
    use crate::query_results::Payload;

    let (root, definitions) = fixture();
    let repo = base_repository(&root, definitions);
    let index = crate::lifecycle::Index::from_repository(repo).unwrap();
    let pinned = index.view().unwrap();
    let handle = pinned
        .file_handle_for_path(std::path::Path::new("entries/a.row"))
        .unwrap();
    let mut request = query("entries", Some("id = 'A' SORT BY title"), None, None, false);
    request.options.return_mode = ReturnMode::Locator;
    let old_lookup = pinned.search_reference(&request).unwrap();
    assert_eq!(old_lookup.count, Some(1));
    let old_path = old_lookup.hits[0].file.as_ref().unwrap().path.clone();
    assert_eq!(old_path, std::path::Path::new("entries/a.row"));

    let original = root.join("entries/a.row");
    let destination = root.join("entries/renamed-a.row");
    let original_bytes = std::fs::read(&original).unwrap();
    std::fs::rename(&original, &destination).unwrap();
    let moved = index.move_file(handle, &destination).unwrap();
    assert_eq!(index.view().unwrap().revision(), moved.revision);

    let current = index.view().unwrap();
    request.options.return_mode = ReturnMode::Bytes;
    let current = current.search_reference(&request).unwrap();
    assert_eq!(current.count, Some(1));
    assert_eq!(
        current.hits[0].file.as_ref().unwrap().path,
        std::path::Path::new("entries/renamed-a.row")
    );
    assert_eq!(
        current.hits[0].payload,
        Some(Payload::Bytes(original_bytes))
    );

    request.options.return_mode = ReturnMode::Bytes;
    let old_bytes = pinned.search_reference(&request).unwrap();
    assert_eq!(old_bytes.count, None);
    assert!(old_bytes.hits.is_empty());
    assert_eq!(old_path, std::path::Path::new("entries/a.row"));
    assert!(old_bytes.counters.source_unavailable_events > 0);
    assert!(
        old_bytes
            .counters
            .source_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.file_path == std::path::Path::new("entries/a.row"))
    );

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopted_base_view_reference_search_handles_a_configured_empty_collection() {
    let (root, mut definitions) = fixture();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let mut empty_schema = Schema::default();
    field(&mut empty_schema, "id", LogicalType::Id, false);
    definitions.push(definition("empty", empty_schema));
    let repo = base_repository(&root, definitions);
    let index = crate::lifecycle::Index::from_repository(repo).unwrap();
    let view = index.view().unwrap();
    let request = query("empty", None, None, None, false);
    let result = view.search_reference(&request).unwrap();
    assert_eq!(result.count, Some(0));
    assert!(result.hits.is_empty());
    assert_eq!(result.counters.scanned_records, 0);

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn base_related_sort_declines_an_incompatible_stored_key() {
    use crate::index::sorting::{SortConfig, SortOptions};
    use crate::model::FieldPath;

    let root = std::env::temp_dir().join(format!(
        "objekte-base-related-sort-collation-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    for directory in ["sources", "targets"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    for (id, title) in [("T2", "item2"), ("T10", "item10")] {
        std::fs::write(
            root.join("targets").join(format!("{id}.row")),
            format!(r#"("id"="{id}","title"="{title}")"#),
        )
        .unwrap();
    }
    for (id, target) in [("S2", "T2"), ("S10", "T10")] {
        std::fs::write(
            root.join("sources").join(format!("{id}.row")),
            format!(r#"("id"="{id}","entry"="{target}")"#),
        )
        .unwrap();
    }
    let mut targets = Schema::default();
    field(&mut targets, "id", LogicalType::Id, false);
    field(&mut targets, "title", LogicalType::String, false);
    let mut target = definition("targets", targets);
    let title = FieldPath::parse("title").unwrap();
    target.sort_fields.push(title.clone());
    target.sort_overrides.insert(
        title,
        SortConfig {
            locale: "en".into(),
            options: SortOptions {
                numeric: true,
                ..Default::default()
            },
        },
    );
    let mut sources = Schema::default();
    field(&mut sources, "id", LogicalType::Id, false);
    field(&mut sources, "entry", LogicalType::Id, false);
    relation(&mut sources, "entry", "targets", false, "sources");
    let repo = base_repository(&root, vec![target, definition("sources", sources)]);
    let index = crate::lifecycle::Index::from_repository(repo).unwrap();
    let view = index.view().unwrap();

    let mut related = query("sources", None, None, None, false);
    related.options.sort = vec![crate::query::options::SortField::computed(
        crate::query::options::SortExpression {
            path: "entry.title".into(),
            binding: None,
            reducer: None,
        },
        false,
    )];
    let optimized = view.search(&related).unwrap();
    let reference = view.search_reference(&related).unwrap();
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["S10", "S2"]
    );
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(reference.count, Some(2));
    assert_eq!(optimized.counters.sort_target_reads, 2);

    let mut direct = query("targets", None, None, None, false);
    direct.options.sort = vec![crate::query::options::SortField::simple("title", false)];
    assert_eq!(
        view.search(&direct)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["T2", "T10"]
    );

    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn base_reference_resolves_omitted_id_map_targets_and_materializes_their_bytes() {
    use crate::model::Value;
    use crate::query_results::Payload;
    let (root, definitions) = fixture();
    let mut repo = base_repository(&root, definitions);
    repo.collections[0]
        .id_lookup
        .remove(&crate::model::RecordId("A".into()));

    let filter = query("mentions", Some("entry.title = 'Hymn'"), None, None, false);
    let reference = repo.search_reference(&filter).unwrap();
    assert_eq!(reference.count, Some(1));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m1"]
    );
    let related = query(
        "mentions",
        Some("(id = 'm1' || id = 'm2') SORT BY entry.title"),
        None,
        None,
        false,
    );
    let locator = repo.search_reference(&related).unwrap();
    assert_eq!(locator.count, Some(2));
    assert!(locator.hits.iter().all(|hit| hit.payload.is_none()));
    let mut logical = related.clone();
    logical.options.return_mode = ReturnMode::Logical;
    let logical = repo.search_reference(&logical).unwrap();
    assert_eq!(
        logical
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert!(logical.hits.iter().all(|hit| matches!(
        &hit.payload,
        Some(Payload::Logical(Value::Object(fields)))
            if fields.get("id") == Some(&Value::Id(hit.record_id.clone()))
    )));
    let mut bytes = related.clone();
    bytes.options.return_mode = ReturnMode::Bytes;
    let bytes = repo.search_reference(&bytes).unwrap();
    assert_eq!(
        bytes
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m2", "m1"]
    );
    assert!(bytes.hits.iter().all(|hit| matches!(
        &hit.payload,
        Some(Payload::Bytes(source))
            if String::from_utf8_lossy(source).contains(&format!(r#""id"="{}""#, hit.record_id))
    )));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn base_reference_multi_target_backlinks_scan_each_relation_once() {
    use crate::model::CollectionId;
    use crate::schema::relations::RelationId;

    let (root, definitions) = fixture();
    let repo = base_repository(&root, definitions);
    let relation_index = repo.collections[1]
        .relations
        .iter()
        .position(|relation| relation.declaration.path == ["contributors"])
        .expect("contributors relation");
    let relation = RelationId {
        source: CollectionId(1),
        ordinal: u32::try_from(relation_index).unwrap(),
    };
    let view = MergedRelationView::new(&repo, &repo.active, &repo.segments, &repo.catalog).unwrap();
    let mut work = RelationWork::default();
    assert_eq!(
        view.reference_backlink_sources(relation, "A", Some(&mut work))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        view.reference_backlink_sources(relation, "B", Some(&mut work))
            .unwrap()
            .len(),
        1
    );
    let visible_sources = repo.collections[1]
        .generation
        .records
        .iter()
        .filter(|record| record.class == crate::inventory::RecordClass::Logical)
        .count() as u64;
    assert_eq!(work.source_reads, visible_sources);
    assert!(work.bytes_read > 0);
    drop(view);

    let request = query(
        "entries",
        Some("@backlinks.contributions.title = 'Both'"),
        None,
        None,
        false,
    );
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert!(reference.counters.relation_bytes_read > 0);
    assert_eq!(
        repo.search(&request)
            .unwrap()
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );

    std::fs::remove_file(root.join("mentions/m1.row")).unwrap();
    let incomplete =
        MergedRelationView::new(&repo, &repo.active, &repo.segments, &repo.catalog).unwrap();
    let mut first_work = RelationWork::default();
    let first = incomplete
        .reference_backlink_sources_with_completeness(relation, "A", Some(&mut first_work))
        .unwrap();
    assert!(!first.complete);
    let diagnostics = incomplete.source_error_events();
    assert!(diagnostics > 0);
    let mut cached_work = RelationWork::default();
    let cached = incomplete
        .reference_backlink_sources_with_completeness(relation, "B", Some(&mut cached_work))
        .unwrap();
    assert!(!cached.complete);
    assert_eq!(cached_work.source_reads, 0);
    assert_eq!(incomplete.source_error_events(), diagnostics);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reference_backlink_sort_streams_discovery_without_target_cache_budget() {
    use crate::model::CollectionId;

    const LARGE_SOURCES: usize = 9;
    const LARGE_RECORD_BYTES: usize = 2 * 1024 * 1024;

    let (root, definitions) = fixture();
    std::fs::write(
        root.join("mentions/m8.row"),
        r#"("id"="m8","contributors"=["A"],"title"="Alpha")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m9.row"),
        r#"("id"="m9","contributors"=["B"],"title"="Zulu")"#,
    )
    .unwrap();
    for number in 0..LARGE_SOURCES {
        let prefix = format!(r#"("id"="unlinked-{number}","entry"="Z""#);
        let suffix = ")";
        let padding = LARGE_RECORD_BYTES
            .saturating_sub(prefix.len() + suffix.len())
            .max(1);
        std::fs::write(
            root.join(format!("mentions/unlinked-{number:02}.row")),
            format!("{prefix}{}{suffix}", " ".repeat(padding)),
        )
        .unwrap();
    }
    let index = crate::lifecycle::Index::start_build(
        &root,
        definitions.clone(),
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    let request = query(
        "entries",
        Some("SORT BY MIN(@backlinks.contributions.title)"),
        None,
        None,
        false,
    );

    let view = MergedRelationView::new(
        pinned.repository(),
        &pinned.0.active,
        &pinned.0.segments,
        &pinned.0.catalog,
    )
    .unwrap();
    let mut old_path = crate::query::sort_binding::MergedSortTargetCache::default();
    let mut old_path_error = None;
    for address in view
        .visible_addresses(CollectionId(1))
        .unwrap()
        .into_iter()
        .filter(|address| {
            view.record(*address)
                .unwrap()
                .is_some_and(|record| record.record.class == crate::inventory::RecordClass::Logical)
        })
    {
        if let Err(error) = old_path.load_source(&view, address) {
            old_path_error = Some(error);
            break;
        }
    }
    assert!(
        old_path_error
            .as_deref()
            .is_some_and(|error| error.contains("sort target cache budget exceeded")),
        "fixture must exceed the former retained sort-cache path: {old_path_error:?}"
    );
    let visible_logical_sources = view
        .visible_addresses(CollectionId(1))
        .unwrap()
        .into_iter()
        .filter(|address| {
            view.record(*address)
                .unwrap()
                .is_some_and(|record| record.record.class == crate::inventory::RecordClass::Logical)
        })
        .count() as u64;
    drop(view);

    let observations = crate::query_execution::runner::arm_reference_row_visits();
    let reference = pinned.search_reference(&request).unwrap();
    assert_eq!(observations.finish(), ["A", "B"]);
    assert_eq!(reference.count, Some(2));
    assert_eq!(
        reference
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(
        reference.counters.relation_source_reads,
        visible_logical_sources
    );
    assert!(reference.counters.relation_bytes_read > 0);
    assert_eq!(reference.counters.sort_target_reads, 3);

    let unavailable_path = root.join("mentions/unlinked-00.row");
    std::fs::remove_file(&unavailable_path).unwrap();
    let unavailable = pinned.search_reference(&request).unwrap();
    assert!(
        unavailable.hits.is_empty(),
        "incomplete backlink discovery must exclude every dependent row: {:?}",
        unavailable.counters
    );
    assert_eq!(unavailable.count, None);
    assert_eq!(unavailable.counters.total_matches, 0);
    assert!(unavailable.counters.source_unavailable_events > 0);
    assert!(!unavailable.counters.match_count_complete);
    assert!(
        unavailable
            .counters
            .source_diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic.file_path == std::path::Path::new("mentions/unlinked-00.row")
            })
    );

    let filtered = query(
        "entries",
        Some(
            "@backlinks.contributions.title = 'Both' \
             SORT BY MIN(@backlinks.contributions.title), title",
        ),
        None,
        None,
        false,
    );
    let filtered = pinned.search_reference(&filtered).unwrap();
    assert!(filtered.hits.is_empty());
    assert_eq!(filtered.count, None);
    assert_eq!(filtered.counters.total_matches, 0);
    assert!(!filtered.counters.match_count_complete);

    let nested = query(
        "entries",
        Some("SORT BY MIN(@backlinks.contributions.related_entry.title)"),
        None,
        None,
        false,
    );
    let nested = pinned.search_reference(&nested).unwrap();
    assert!(nested.hits.is_empty());
    assert_eq!(nested.count, None);
    assert_eq!(nested.counters.total_matches, 0);
    assert!(!nested.counters.match_count_complete);

    let independent = query(
        "entries",
        Some("SORT BY MIN(@backlinks.related_from.title)"),
        None,
        None,
        false,
    );
    let independent = pinned.search_reference(&independent).unwrap();
    assert_eq!(
        independent
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(independent.count, Some(2));
    assert!(independent.counters.match_count_complete);

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();

    let reverse_root = std::env::temp_dir().join(format!(
        "objekte-reference-backlink-reverse-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    for directory in ["entries", "mentions"] {
        std::fs::create_dir_all(reverse_root.join(directory)).unwrap();
    }
    for (relative, document) in [
        ("entries/00-b.row", r#"("id"="B","title"="Bravo")"#),
        ("entries/50-c.row", r#"("id"="C","title"="Charlie")"#),
        ("entries/99-a.row", r#"("id"="A","title"="Zulu")"#),
        (
            "mentions/m1.row",
            r#"("id"="m1","entry"="A","title"="Zulu")"#,
        ),
        (
            "mentions/m2.row",
            r#"("id"="m2","entry"="B","title"="Bravo")"#,
        ),
        (
            "mentions/z-unlinked.row",
            r#"("id"="unlinked","entry"="Z")"#,
        ),
    ] {
        std::fs::write(reverse_root.join(relative), document).unwrap();
    }
    let reverse = crate::lifecycle::Index::start_build(
        &reverse_root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let reverse_pin = reverse.view().unwrap();
    std::fs::remove_file(reverse_root.join("mentions/z-unlinked.row")).unwrap();
    let observations = crate::query_execution::runner::arm_reference_row_visits();
    let reverse_request = query(
        "entries",
        Some(
            "(id = 'A' || id = 'B' || id = 'C') \
             SORT BY MIN(@backlinks.mentions.title)",
        ),
        None,
        None,
        false,
    );
    let reverse_result = reverse_pin.search_reference(&reverse_request).unwrap();
    assert_eq!(observations.finish(), ["B", "C", "A"]);
    assert!(reverse_result.hits.is_empty());
    assert_eq!(reverse_result.count, None);
    assert_eq!(reverse_result.counters.total_matches, 0);
    assert!(!reverse_result.counters.match_count_complete);
    assert!(
        reverse_result
            .counters
            .source_diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic.file_path == std::path::Path::new("mentions/z-unlinked.row")
            })
    );

    drop(reverse_pin);
    reverse.shutdown().unwrap();
    std::fs::remove_dir_all(reverse_root).unwrap();
}

#[test]
fn incomplete_backlink_discovery_stops_before_target_sort_cache_work() {
    const RECORD_BYTES: usize = 2 * 1024 * 1024;
    const LINKED_SOURCES: usize = 8;

    let root = std::env::temp_dir().join(format!(
        "objekte-incomplete-backlink-short-circuit-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    for directory in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Target")"#).unwrap();
    for number in 0..LINKED_SOURCES {
        let prefix = format!(r#"("id"="s{number}","contributors"=["A"],"title"="T{number}""#);
        let suffix = ")";
        let padding = RECORD_BYTES - prefix.len() - suffix.len();
        let document = format!("{prefix}{}{suffix}", " ".repeat(padding));
        assert_eq!(document.len(), RECORD_BYTES);
        std::fs::write(root.join(format!("mentions/s{number}.row")), document).unwrap();
    }
    std::fs::write(
        root.join("mentions/ninth.row"),
        r#"("id"="ninth","contributors"=["B"],"title"="Unlinked")"#,
    )
    .unwrap();

    let mut entries = Schema::default();
    field(&mut entries, "id", LogicalType::Id, false);
    field(&mut entries, "title", LogicalType::String, false);
    let mut mentions = Schema::default();
    field(&mut mentions, "id", LogicalType::Id, false);
    field(&mut mentions, "title", LogicalType::String, false);
    field(
        &mut mentions,
        "contributors",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    relation(
        &mut mentions,
        "contributors",
        "entries",
        true,
        "contributions",
    );
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![
            definition("entries", entries),
            definition("mentions", mentions),
        ],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..RepositoryOptions::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let pinned = index.view().unwrap();
    std::fs::remove_file(root.join("mentions/ninth.row")).unwrap();
    let request = query(
        "entries",
        Some("SORT BY MIN(@backlinks.contributions.title)"),
        None,
        None,
        false,
    );
    let owner_view = MergedRelationView::new(
        pinned.repository(),
        &pinned.0.active,
        &pinned.0.segments,
        &pinned.0.catalog,
    )
    .unwrap();
    let owner_address = owner_view
        .resolve_id(crate::model::CollectionId(0), "A")
        .unwrap()
        .unwrap();
    let owner_value = owner_view
        .reference_value(owner_address, None)
        .unwrap()
        .unwrap();
    let owner_record = owner_view.record(owner_address).unwrap().unwrap();
    let owner_ctx = crate::query::semantics::record::RecordContext {
        value: &owner_value,
        source_bytes: None,
        record: &owner_record.record,
        range: (owner_record.record).record_range(),
        file: Some(owner_record.file),
        collection_name: &owner_record.collection.definition.name,
        repository_root: Some(owner_view.repository_root()),
    };
    let owner_sort =
        crate::query_results::ranking::ResolvedSort::new_reference(pinned.repository(), &request)
            .unwrap();
    let mut owner_cache = crate::query::sort_binding::MergedSortTargetCache::default();
    let owner_key = crate::query_results::ranking::relation_rank_key_merged(
        pinned.repository(),
        &owner_sort,
        &owner_view,
        &pinned.0.segments,
        owner_address,
        &owner_ctx,
        &mut owner_cache,
        None,
        true,
    )
    .unwrap();
    assert!(owner_key.is_none());
    assert_eq!(owner_cache.relation_source_reads, LINKED_SOURCES as u64);
    assert_eq!(
        owner_cache.relation_source_bytes,
        (LINKED_SOURCES * RECORD_BYTES) as u64
    );
    assert_eq!(owner_cache.reads, 0);
    let result = pinned.search_reference(&request).unwrap();
    assert!(result.hits.is_empty());
    assert_eq!(result.count, None);
    assert_eq!(result.counters.total_matches, 0);
    assert!(!result.counters.match_count_complete);
    assert_eq!(result.counters.sort_target_reads, 0);
    assert_eq!(
        result.counters.relation_source_reads, LINKED_SOURCES as u64,
        "{:?}",
        result.counters
    );
    assert_eq!(
        result.counters.relation_bytes_read,
        (LINKED_SOURCES * RECORD_BYTES) as u64
    );
    assert_eq!(result.counters.files_opened, 9);
    assert_eq!(result.counters.record_parses, 9);
    assert_eq!(result.counters.bytes_read, 16_777_243);
    assert!(
        result.counters.source_diagnostics.iter().any(|diagnostic| {
            diagnostic.file_path == std::path::Path::new("mentions/ninth.row")
        })
    );

    drop(pinned);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_parallel_relation_files_preserve_answers_and_missing_target_diagnostics() {
    use crate::query_execution::streaming::{
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
            crate::source::test_support::fixture_registry(),
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
    std::fs::remove_file(root.join("entries/a.row")).unwrap();
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
            root.join(format!("entries/extra-{number:03}.row")),
            format!(r#"("id"="E{number:03}","title"="Hymn {number}")"#),
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
            crate::source::test_support::fixture_registry(),
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
    std::fs::remove_file(root.join("entries/a.row")).unwrap();
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
            "entries/a.row",
            r#"("id"="A","title"="Hymn","alt_title"="Hymn","related_entry"="B")"#,
        ),
        (
            "entries/b.row",
            r#"("id"="B","title"="Festival","alt_title"="Other","related_entry"="A")"#,
        ),
        (
            "mentions/m1.row",
            r#"("id"="m1","entry"="A","items"=[("entry"="A"),("entry"="B")],"title"="Festival","local_title"="Hymn")"#,
        ),
        (
            "mentions/m2.row",
            r#"("id"="m2","entry"="B","items"=[("entry"="B")],"title"="Hymn","local_title"="x")"#,
        ),
        (
            "mentions/m3.row",
            r#"("id"="m3","entry"="Z","title"="Dangling")"#,
        ),
        (
            "mentions/m4.row",
            r#"("id"="m4","entry"=null,"title"="Null")"#,
        ),
        ("mentions/m5.row", r#"("id"="m5","title"="Missing")"#),
        (
            "mentions/m6.row",
            r#"("id"="m6","contributors"=["A","B"],"title"="Both")"#,
        ),
        (
            "mentions/m7.row",
            r#"("id"="m7","contributors"=[],"title"="Empty")"#,
        ),
        ("mentions/broken.row", "(broken"),
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

fn base_repository(root: &std::path::Path, definitions: Vec<CollectionDefinition>) -> Repository {
    let mut repo = crate::lifecycle::Index::build_repository_for_test(
        root,
        definitions,
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let file_count = repo.files.files.len();
    let mut relation_contributions = Vec::with_capacity(repo.collections.len());
    for collection_index in 0..repo.collections.len() {
        if repo.segments[collection_index].is_empty() {
            assert_eq!(
                repo.collections[collection_index].generation.records.len(),
                0
            );
            repo.active[collection_index] = crate::inventory::ActiveRecords::all(0).unwrap();
            relation_contributions.push(Vec::new());
            continue;
        }
        assert_eq!(
            repo.segments[collection_index].len(),
            1,
            "test fixture expects one native segment per collection"
        );
        let segment = Arc::clone(&repo.segments[collection_index][0].index);
        let records = (0..segment.records.len())
            .map(|slot| {
                segment
                    .file_record(crate::model::RecordSlot(slot as u32))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let files = segment
            .files
            .iter()
            .map(|file| file.entry.id)
            .collect::<Vec<_>>();
        let mut slots_by_file = vec![Vec::new(); file_count];
        for record in &records {
            slots_by_file[record.file.0 as usize].push(record.slot);
        }
        let mut offsets = Vec::with_capacity(file_count + 1);
        let mut slots = Vec::with_capacity(records.len());
        offsets.push(0);
        for file_slots in slots_by_file {
            slots.extend(file_slots);
            offsets.push(u32::try_from(slots.len()).unwrap());
        }

        let state = &mut repo.collections[collection_index];
        state.generation.files = files;
        state.generation.records = records;
        state.generation.file_records = crate::inventory::FileRecordIndex {
            file_count: u32::try_from(file_count).unwrap(),
            offsets,
            slots,
        };
        state.postings = (*segment.postings).clone();
        state.inference = segment.inference.clone();
        state.custom_postings = segment.custom_postings.clone();
        state.raw = (*segment.raw).clone();
        state.fallback_text = (*segment.fallback_text).clone();
        state.logical_records = segment.logical_records.clone();
        state.empty_array_records = segment.empty_array_records.clone();
        state.custom_text = segment.custom_text.clone();
        state.sorting = (*segment.sorting).clone();
        state.index_family_bytes = segment.accounting.index_family_bytes;
        state.id_lookup = segment.id_lookup.clone();
        state.identity_order = segment.identity_order.clone();
        state.stats.record_count = state.generation.records.len();
        relation_contributions.push(segment.relations.clone());
        repo.active[collection_index] =
            crate::inventory::ActiveRecords::all(state.generation.records.len()).unwrap();
        repo.segments[collection_index] = Arc::new(Vec::new());
    }
    for (collection_index, contributions) in relation_contributions.into_iter().enumerate() {
        let source = repo.collections[collection_index].generation.collection;
        let relations = contributions
            .into_iter()
            .enumerate()
            .map(|(ordinal, contribution)| {
                contribution.resolve(source, u32::try_from(ordinal).unwrap(), &repo.collections)
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        repo.collections[collection_index].relations = relations;
    }
    repo
}

fn query(
    collection: &str,
    filter: Option<&str>,
    fts: Option<&str>,
    field: Option<&str>,
    regex: bool,
) -> Query {
    query_from_fixture(&QueryFixture {
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

fn emit_relation_probe(
    case_id: &str,
    result: &crate::query_results::SearchResult,
    reference: &crate::query_results::SearchResult,
    trace: &crate::query_diagnostics::trace::QueryPlanTrace,
) {
    if std::env::var_os("OBJEKTE_RELATION_PROBE").is_none() {
        return;
    }
    println!(
        "RELATION_PROBE case_id={case_id} alphabet=Ascii result={result:?} reference: {reference:?} plan={trace:?}"
    );
}

fn emit_relation_timing(case_id: &str, repo: &Repository, request: &Query, repetitions: usize) {
    if std::env::var_os("OBJEKTE_RELATION_TIMING").is_none() {
        return;
    }
    repo.search(request).unwrap();
    let mut samples = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let started = std::time::Instant::now();
        let result = repo.search(request).unwrap();
        assert!(result.count.is_some());
        samples.push(started.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    println!(
        "RELATION_TIMING case_id={case_id} repetitions={repetitions} samples_ns={samples:?} median_ns={}",
        samples[samples.len() / 2]
    );
}

fn emit_segment_repair_timing(
    case_id: &str,
    repetitions: usize,
    mut run: impl FnMut() -> crate::query_results::SearchResult,
) {
    if std::env::var_os("OBJEKTE_REPAIR_TIMING").is_none() {
        return;
    }
    run();
    let mut samples = Vec::with_capacity(repetitions);
    let mut result = None;
    for _ in 0..repetitions {
        let started = std::time::Instant::now();
        let current = run();
        samples.push(started.elapsed().as_nanos() as u64);
        result = Some(current);
    }
    samples.sort_unstable();
    let result = result.unwrap();
    println!(
        "SEGMENT_REPAIR_TIMING case_id={case_id} repetitions={repetitions} samples_ns={samples:?} median_ns={} result={result:?}",
        samples[samples.len() / 2]
    );
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
            root.join(format!("mentions/bulk-{index:03}.row")),
            format!(r#"("id"="bulk-{index:03}","entry"="A","local_title"="Hymn","title"="bulk")"#),
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
        crate::source::test_support::fixture_registry(),
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
    std::fs::write(root.join("entries/a.row"), r#"("id"="A","title"="Alpha")"#).unwrap();
    std::fs::write(root.join("entries/b.row"), r#"("id"="B","title"="Zulu")"#).unwrap();
    std::fs::write(root.join("entries/c.row"), r#"("id"="C","title"="Middle")"#).unwrap();
    for (name, values) in [
        ("order_bab", "[\"B\",\"A\",\"B\"]"),
        ("order_a", "[\"A\"]"),
        ("order_b", "[\"B\"]"),
        ("order_m", "[\"C\"]"),
    ] {
        std::fs::write(
            root.join(format!("mentions/{name}.row")),
            format!(r#"("id"="{name}","contributors"={values})"#),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
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
        root.join("entries/a.row"),
        r#"("id"="A","title"="Hymn","alt_title"="Zulu")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("entries/b.row"),
        r#"("id"="B","title"="Festival","alt_title"="Alpha")"#,
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/m8.row"),
        r#"("id"="m8","entry"="B","local_title"="Festival")"#,
    )
    .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
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
    emit_relation_timing("source_dependent_residual_sort", &repo, &sorted, 21);
    let reference = repo.search_reference(&sorted).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.counters.sort_target_reads, 2);
    let trace = repo.explain_analyze(&sorted).unwrap().1;
    emit_relation_probe(
        "source_dependent_residual_sort",
        &result,
        &reference,
        &trace[0],
    );

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
        root.join("entries/a.row"),
        format!(
            r#"("id"="A","title"="Hymn","alt_title"="{}")"#,
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
        crate::source::test_support::fixture_registry(),
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
            &mut |_| Ok(crate::query_execution::streaming::StreamControl::Continue),
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
        crate::source::test_support::fixture_registry(),
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
    sort.options.sort.push(crate::query::options::SortField {
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
fn bound_filter_leaf_evidence_has_fixed_expected_fields() {
    let (root, definitions) = fixture();
    let repo = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions,
            RepositoryOptions {
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    let bind_term = |filter: &str| {
        let request = query("mentions", Some(filter), None, None, false);
        let prepared = crate::query::prepared::prepare(&repo, &request).unwrap();
        let crate::query::binding::filter::BoundExpr::Term(term) =
            prepared.collections[0].filter().unwrap()
        else {
            panic!("expected one bound term")
        };
        term.clone()
    };

    let mut fields = std::collections::HashSet::new();
    let field_comparison = bind_term("entry.title = local_title");
    crate::query::semantics::filter::emit_bound_filter_evidence(
        &field_comparison,
        Some(true),
        Some(&mut fields),
    );
    assert_eq!(
        fields,
        std::collections::HashSet::from(["entry.title".into(), "local_title".into()])
    );

    fields.clear();
    let relation_comparison = bind_term("entry.title = 'Hymn'");
    crate::query::semantics::filter::emit_bound_filter_evidence(
        &relation_comparison,
        Some(true),
        Some(&mut fields),
    );
    assert_eq!(
        fields,
        std::collections::HashSet::from(["entry.title".into()])
    );

    fields.clear();
    let negative = bind_term("entry.title != 'Hymn'");
    crate::query::semantics::filter::emit_bound_filter_evidence(
        &negative,
        Some(true),
        Some(&mut fields),
    );
    assert!(fields.is_empty());

    let mut derived = relation_comparison.clone();
    derived.derived = crate::query::binding::filter::Derived::Length;
    crate::query::semantics::filter::emit_bound_filter_evidence(
        &derived,
        Some(true),
        Some(&mut fields),
    );
    assert!(fields.is_empty());

    crate::query::semantics::filter::emit_bound_filter_evidence(
        &relation_comparison,
        None,
        Some(&mut fields),
    );
    assert!(fields.is_empty());

    let request = query(
        "mentions",
        Some("entry.title = local_title"),
        None,
        None,
        true,
    );
    let prepared = crate::query::prepared::prepare(&repo, &request).unwrap();
    let filter = prepared.collections[0].filter().unwrap();
    let collection_index = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == "mentions")
        .unwrap();
    let collection = &repo.collections[collection_index];
    let slot = collection.id_lookup["m1"];
    let record = &collection.generation.records[slot.0 as usize];
    let file = &repo.files.files[record.file.0 as usize];
    let path = repo.files.native_path(record.file).unwrap();
    let bytes = crate::source::access::read_record_range_for_search(
        &crate::source::test_support::DOCUMENT_READER,
        &path,
        record.revision,
        record.range,
        None,
    )
    .unwrap();
    let extracted = crate::source::preparation::prepare_query_record_with_execution(
        record.class,
        &collection.required_file_reader(),
        &collection.definition.schema,
        &bytes,
        &crate::source::ScanProjection::full(true),
        false,
        None,
    )
    .unwrap();
    let base_ctx = crate::query::semantics::record::RecordContext {
        value: &extracted.value,
        source_bytes: Some(&bytes),
        record,
        range: (record).record_range(),
        file: Some(file),
        collection_name: &collection.definition.name,
        repository_root: Some(repo.files.required_root()),
    };
    let mut base_fields = std::collections::HashSet::new();
    let mut work = RelationWork::default();
    assert_eq!(
        super::eval_filter_truth(
            &repo,
            filter,
            &base_ctx,
            &mut base_fields,
            super::EvaluationMode::Indexed,
            &mut work,
        )
        .unwrap(),
        Some(true)
    );

    let catalog =
        crate::lifecycle::catalog::FileCatalog::from_repository(Arc::clone(&repo)).unwrap();
    let merged = MergedRelationView::new(&repo, &repo.active, &repo.segments, &catalog).unwrap();
    let address = merged
        .resolve_id(crate::model::CollectionId(collection_index as u32), "m1")
        .unwrap()
        .unwrap();
    let value = merged.reference_value(address, None).unwrap().unwrap();
    let resolved = merged.record(address).unwrap().unwrap();
    let merged_ctx = crate::query::semantics::record::RecordContext {
        value: &value,
        source_bytes: None,
        record: &resolved.record,
        range: (resolved.record).record_range(),
        file: Some(resolved.file),
        collection_name: &resolved.collection.definition.name,
        repository_root: Some(merged.repository_root()),
    };
    let mut merged_fields = std::collections::HashSet::new();
    let mut work = RelationWork::default();
    assert_eq!(
        super::eval_filter_truth_merged(
            &merged,
            filter,
            address,
            &merged_ctx,
            &mut merged_fields,
            &mut work,
            false,
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        base_fields,
        std::collections::HashSet::from(["entry.title".into(), "local_title".into()])
    );
    assert_eq!(merged_fields, base_fields);
    let residual = crate::query_predicate::prepare_residual(
        filter,
        &[],
        collection,
        collection.generation.records.len() as u64,
        &prepared.predicates,
    );
    let mut residual_fields = std::collections::HashSet::new();
    let mut residual_evaluations = 0;
    let mut work = RelationWork::default();
    assert_eq!(
        residual
            .with_evidence(
                &repo,
                slot.0,
                &base_ctx,
                &mut residual_fields,
                &mut residual_evaluations,
                &mut work,
                None,
            )
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        residual_fields,
        std::collections::HashSet::from(["entry.title".into(), "local_title".into()])
    );

    let reverse = query(
        "mentions",
        Some("local_title = entry.title"),
        None,
        None,
        true,
    );
    let prepared_reverse = crate::query::prepared::prepare(&repo, &reverse).unwrap();
    let filter_reverse = prepared_reverse.collections[0].filter().unwrap();
    let mut reverse_merged = std::collections::HashSet::new();
    let mut work = RelationWork::default();
    assert_eq!(
        super::eval_filter_truth_merged(
            &merged,
            filter_reverse,
            address,
            &merged_ctx,
            &mut reverse_merged,
            &mut work,
            false,
        )
        .unwrap(),
        Some(true)
    );
    let reverse_residual = crate::query_predicate::prepare_residual(
        filter_reverse,
        &[],
        collection,
        collection.generation.records.len() as u64,
        &prepared_reverse.predicates,
    );
    let mut reverse_residual_fields = std::collections::HashSet::new();
    let mut residual_evaluations = 0;
    let mut work = RelationWork::default();
    assert_eq!(
        reverse_residual
            .with_evidence(
                &repo,
                slot.0,
                &base_ctx,
                &mut reverse_residual_fields,
                &mut residual_evaluations,
                &mut work,
                None,
            )
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        reverse_merged,
        std::collections::HashSet::from(["entry.title".into(), "local_title".into()])
    );
    assert_eq!(reverse_residual_fields, reverse_merged);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn broad_relation_or_filter_uses_filter_only_pruning() {
    let (root, definitions) = fixture();
    let repo = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions,
            RepositoryOptions {
                verified_cache_bytes: 0,
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    let catalog =
        crate::lifecycle::catalog::FileCatalog::from_repository(Arc::clone(&repo)).unwrap();
    let base_active = repo.active.clone();
    let revision = crate::lifecycle::IndexRevision(1);
    let request = query(
        "mentions",
        Some("@file.status = 1 || entry.title = 'Hymn'"),
        None,
        None,
        true,
    );
    let result = crate::query_execution::runner::search_segmented(
        &repo,
        &request,
        &base_active,
        &repo.segments,
        &catalog,
        revision,
    )
    .unwrap();
    emit_segment_repair_timing("broad_filter_no_relation_prune", 21, || {
        crate::query_execution::runner::search_segmented(
            &repo,
            &request,
            &base_active,
            &repo.segments,
            &catalog,
            revision,
        )
        .unwrap()
    });
    let reference = crate::query_execution::runner::search_segmented_reference(
        &repo,
        &request,
        &base_active,
        &repo.segments,
        &catalog,
        revision,
    )
    .unwrap();
    assert_eq!(result.count, reference.count);
    assert_eq!(result.hits, reference.hits);
    let traces = crate::query_execution::runner::explain_segmented(
        &repo,
        &request,
        &base_active,
        &repo.segments,
        revision,
    )
    .unwrap();
    assert!(traces[0].terms.iter().all(|term| !matches!(
        term.selected.kind,
        crate::query_execution::restrictions::AccessPathKind::RelationTargetFirst
            | crate::query_execution::restrictions::AccessPathKind::RelationSourceRestricted
    )));
    std::fs::remove_dir_all(root).unwrap();
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
        crate::source::test_support::fixture_registry(),
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
        crate::source::test_support::fixture_registry(),
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
        crate::source::test_support::fixture_registry(),
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
        crate::source::test_support::fixture_registry(),
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
        crate::source::test_support::fixture_registry(),
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
        crate::source::test_support::fixture_registry(),
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
