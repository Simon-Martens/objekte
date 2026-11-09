use std::path::PathBuf;
use std::sync::Arc;

use crate::model::LogicalType;
use crate::query::{CaseSensitivity, FtsMode, Query};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;

#[test]
fn merged_relations_resolve_replacement_targets_and_preserve_unknown() {
    use crate::build::{
        FilePreparationBinding, IndexedCollectionSegment, SegmentAdmission, SourceFileInput,
        prepare_file_build,
    };
    use crate::inventory::{ActiveRecords, SourceRevision};
    use crate::lifecycle::{IndexRevision, PublishedSegment};
    use crate::lifecycle_catalog::FileCatalog;
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
                created_ns: metadata.created().ok()
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
            .seal()
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
        Repository::open_with_options(
            &root,
            definitions,
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
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
    let missing = crate::run::search_segmented(
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
            ordinal: 1,
            active: ActiveRecords::all(target.records.len()).unwrap(),
            index: Arc::new(target),
        })]),
        Arc::new(vec![Arc::new(PublishedSegment {
            ordinal: 2,
            active: ActiveRecords::all(source.records.len()).unwrap(),
            index: Arc::new(source),
        })]),
    ];
    let find = |request: Query| {
        crate::run::search_segmented(
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
        &repo, "mentions", &["entry".into()], "m8", 0, 1,
        IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(outgoing.source.segment_ordinal, Some(2));
    assert_eq!(outgoing.total_occurrences, 1);
    assert_eq!(outgoing.references[0].resolved.as_ref().unwrap().id, "A");
    assert_eq!(outgoing.references[0].resolved.as_ref().unwrap().segment_ordinal, Some(1));
    let nested = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo, "mentions", &["items".into(), "entry".into()], "m1", 0, 1,
        IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(nested.total_occurrences, 2);
    assert_eq!(nested.next_offset, Some(1));
    assert_eq!(nested.references[0].positions, [0]);
    assert_eq!(nested.references[0].resolved.as_ref().unwrap().segment_ordinal, Some(1));
    let null = crate::relation_inspection::relation_outgoing_page_segmented(
        &repo, "mentions", &["entry".into()], "m4", 0, 1,
        IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(null.presence, crate::relation_inspection::RelationPresence::Present);
    assert!(null.contains_null);
    assert_eq!(null.total_occurrences, 0);
    let backlinks = crate::relation_inspection::relation_backlink_page_segmented(
        &repo, "entries", "mentions", "A", 0, 1,
        IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(backlinks.total_sources, 2);
    assert_eq!(backlinks.next_offset, Some(1));
    assert_eq!(backlinks.sources[0].id, "m1");
    let backlink_tail = crate::relation_inspection::relation_backlink_page_segmented(
        &repo, "entries", "mentions", "A", 1, 1,
        IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(backlink_tail.sources[0].id, "m8");
    assert_eq!(backlink_tail.sources[0].segment_ordinal, Some(2));
    let graph = crate::relation_inspection::collection_graph_segmented(
        &repo, IndexRevision(3), &active, &segments, &catalog,
    ).unwrap();
    assert_eq!(graph.nodes[0].record_count, 2);
    assert_eq!(graph.nodes[1].generation, 3);
    let mentions_edge = graph.edges.iter().find(|edge| {
        edge.reverse_alias.as_deref() == Some("mentions")
    }).unwrap();
    assert_eq!(mentions_edge.occurrence_count, 4);
    assert_eq!(mentions_edge.unresolved_occurrence_count, 1);
    let mut retired_source = (*segments[1][0]).clone();
    retired_source.active = retired_source.active
        .retired(&[crate::model::RecordSlot(0)]).unwrap();
    let without_source = vec![Arc::clone(&segments[0]), Arc::new(vec![Arc::new(retired_source)])];
    let source_retired_backlinks = crate::relation_inspection::relation_backlink_page_segmented(
        &repo, "entries", "mentions", "A", 0, 10,
        IndexRevision(4), &active, &without_source, &catalog,
    ).unwrap();
    assert_eq!(source_retired_backlinks.total_sources, 1);
    assert_eq!(source_retired_backlinks.sources[0].id, "m1");
    let source_retired_graph = crate::relation_inspection::collection_graph_segmented(
        &repo, IndexRevision(4), &active, &without_source, &catalog,
    ).unwrap();
    assert_eq!(source_retired_graph.edges.iter().find(|edge| {
        edge.reverse_alias.as_deref() == Some("mentions")
    }).unwrap().occurrence_count, 3);
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
        crate::workload::SortExpression {
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
        crate::workload::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::workload::SortReducer::First),
        },
        false,
    )];
    let first_order = find(first);
    let first_at = |id: &str| first_order.iter().position(|found| found == id).unwrap();
    assert!(first_at("m2") < first_at("m1"));
    let mut last = query("mentions", None, None, None, false);
    last.options.sort = vec![crate::execution_options::SortField::computed(
        crate::workload::SortExpression {
            path: "items.entry.title".into(),
            binding: None,
            reducer: Some(crate::workload::SortReducer::Last),
        },
        false,
    )];
    let last_order = find(last);
    let last_at = |id: &str| last_order.iter().position(|found| found == id).unwrap();
    assert!(last_at("m1") < last_at("m2"));
    let mut backlink_sort = query("entries", None, None, None, false);
    backlink_sort.options.sort = vec![crate::execution_options::SortField::computed(
        crate::workload::SortExpression {
            path: "@backlinks.mentions.title".into(),
            binding: None,
            reducer: Some(crate::workload::SortReducer::Min),
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
        crate::run::search_segmented(
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
        &repo, "mentions", &["entry".into()], "m8", 0, 1,
        IndexRevision(4), &active, &retired_segments, &catalog,
    ).unwrap();
    assert!(unresolved.references[0].resolved.is_none());
    assert!(crate::relation_inspection::relation_backlink_page_segmented(
        &repo, "entries", "mentions", "A", 0, 1,
        IndexRevision(4), &active, &retired_segments, &catalog,
    ).is_err());
    let retired_graph = crate::relation_inspection::collection_graph_segmented(
        &repo, IndexRevision(4), &active, &retired_segments, &catalog,
    ).unwrap();
    let retired_edge = retired_graph.edges.iter().find(|edge| {
        edge.reverse_alias.as_deref() == Some("mentions")
    }).unwrap();
    assert_eq!(retired_edge.unresolved_occurrence_count, 3);
    std::fs::remove_file(root.join("entries/new-a.json")).unwrap();
    let unavailable_target = crate::run::search_segmented(
        &repo,
        &query("mentions", Some("!(entry.title = 'New Hymn')"), None, None, false),
        &active, &segments, &catalog, IndexRevision(3),
    ).unwrap();
    assert_eq!(unavailable_target.count, None);
    assert!(!unavailable_target.counters.match_count_complete);
    assert_eq!(unavailable_target.counters.source_diagnostics[0].kind,
        crate::measurement::SourceUnavailableKind::Missing);
    assert!(!unavailable_target.hits.iter().any(|hit| hit.record_id == "m1" || hit.record_id == "m8"));
    let mut unavailable_sort = query("mentions", None, None, None, false);
    unavailable_sort.options.sort = vec![crate::execution_options::SortField::computed(
        crate::workload::SortExpression {
            path: "entry.title".into(), binding: Some("lower".into()), reducer: None,
        },
        false,
    )];
    let unavailable_sort = crate::run::search_segmented(
        &repo, &unavailable_sort, &active, &segments, &catalog, IndexRevision(3),
    ).unwrap();
    assert_eq!(unavailable_sort.count, None);
    assert!(!unavailable_sort.hits.iter().any(|hit| hit.record_id == "m1" || hit.record_id == "m8"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn merged_relation_walker_matches_static_reference_semantics() {
    use crate::inventory::ActiveRecords;
    use crate::lifecycle::IndexRevision;
    use crate::lifecycle_catalog::FileCatalog;

    let (root, definitions) = fixture();
    let repo = Arc::new(
        Repository::open_with_options(
            &root,
            definitions,
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
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
        let actual = crate::run::search_segmented(
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
        ("mentions", "items.entry.title", Some(crate::workload::SortReducer::First)),
        ("mentions", "items.entry.title", Some(crate::workload::SortReducer::Last)),
        ("mentions", "contributors.title", Some(crate::workload::SortReducer::Min)),
        ("entries", "@backlinks.mentions.title", Some(crate::workload::SortReducer::Min)),
    ] {
        let mut request = query(collection, None, None, None, false);
        request.options.sort = vec![crate::execution_options::SortField::computed(
            crate::workload::SortExpression {
                path: path.into(),
                binding: None,
                reducer,
            },
            false,
        )];
        let expected = repo.search_reference(&request).unwrap();
        let actual = crate::run::search_segmented(
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
            actual.hits.iter().map(|hit| &hit.record_id).collect::<Vec<_>>(),
            expected.hits.iter().map(|hit| &hit.record_id).collect::<Vec<_>>(),
            "{path}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
use crate::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamError, StreamEvent, StreamOptions,
    StreamOutcome,
};
use crate::workload::{ReturnMode, WorkloadCase};

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
    let mut counters = crate::workload::WorkCounters {
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
    use crate::lifecycle::Index;

    let (root, definitions) = fixture();
    let index = Index::build(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    ).unwrap();
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
    std::fs::File::open(&target).unwrap().set_times(
        std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()),
    ).unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&target).unwrap()).unwrap(),
        SourceRevision::from_metadata(&metadata).unwrap(),
    );
    let mut bytes = query(
        "mentions", Some("entry.title = 'Hymn' && @record.id = 'm1'"),
        None, None, false,
    );
    bytes.options.return_mode = ReturnMode::Bytes;
    let stale = view.search(&bytes).unwrap();
    assert_eq!(stale.count, None);
    assert!(!stale.counters.match_count_complete);
    assert!(stale.counters.source_unavailable_events > 0);
    assert!(stale.hits.is_empty());
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_related_sort_uses_target_stored_column() {
    use crate::lifecycle::Index;
    use crate::model::FieldPath;

    let (root, mut definitions) = fixture();
    definitions[0].sort_fields.push(FieldPath::parse("title").unwrap());
    let index = Index::build(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    ).unwrap();
    let view = index.view().unwrap();
    let mut request = query("mentions", Some("entry.title != 'Missing'"), None, None, false);
    request.options.sort = vec![crate::execution_options::SortField::computed(
        crate::workload::SortExpression {
            path: "entry.title".into(), binding: None, reducer: None,
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
    use crate::lifecycle::Index;
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
    let index = Index::build(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )
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
fn lifecycle_parallel_relation_files_preserve_answers_and_missing_target_diagnostics() {
    use crate::lifecycle::Index;
    use crate::streaming::{
        CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits,
    };

    let (root, definitions) = fixture();
    let build = |workers| {
        Index::build(
            &root,
            definitions.clone(),
            RepositoryOptions {
                query_workers: workers,
                snapshot_mode: SnapshotMode::Disabled,
                ..RepositoryOptions::default()
            },
        )
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
        crate::clock_context::ClockContext::new(time::OffsetDateTime::now_utc()),
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
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
    sort.options.sort.push(crate::workload::SortField {
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
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
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let repo = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
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
    repo.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
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
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
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
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn selected_fts_rejects_disagreeing_public_field_and_query_path() {
    let (root, definitions) = fixture();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
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
