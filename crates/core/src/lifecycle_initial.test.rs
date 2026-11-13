use super::*;
use std::sync::mpsc;

use crate::model::FileId;
use crate::query::{Query, QueryOptions, parse_filter};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn all_query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 3_000,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn discovery_and_segments_publish_without_losing_records() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-publish-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..2_050 {
        std::fs::write(
            root.join(format!("data/f{number:04}.json")),
            format!(r#"{{"name":"n{number}"}}"#),
        )
        .unwrap();
    }
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let restore_definition = definition.clone();
    let (catalogue_tx, catalogue_rx) = mpsc::sync_channel(0);
    let (publication_tx, publication_rx) = mpsc::sync_channel(0);
    let (resume_tx, resume_rx) = mpsc::sync_channel(0);
    let worker = std::thread::spawn({
        let root = root.clone();
        move || {
            Repository::build_incremental_with_sources(
                root,
                vec![definition],
                RepositoryOptions {
                    build_workers: std::num::NonZeroUsize::new(4),
                    ..RepositoryOptions::default()
                },
                SourceRegistry::builtin(),
                |index| {
                    catalogue_tx.send(index.clone()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
                |index| {
                    publication_tx.send(index.view().unwrap()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
                || false,
            )
        }
    });
    let index = catalogue_rx.recv().unwrap();
    let catalogue = index.view().unwrap();
    assert!(!catalogue.initial_construction_complete());
    assert_eq!(catalogue.catalogue_rows().count(), 2_050);
    assert_eq!(catalogue.catalogue_row(FileId(0)).unwrap().size > 0, true);
    assert_eq!(catalogue.file_record_count(FileId(0)), None);
    assert!(catalogue.search(&all_query()).is_err());
    assert!(
        catalogue
            .get_record("data", "~generated:data/f0000.json", ReturnMode::Locator)
            .is_err()
    );
    assert!(catalogue.search_reference(&all_query()).is_err());
    assert!(catalogue.explain(&all_query()).is_err());
    assert!(
        index
            .flush_cache(&root.with_extension("premature.idx"))
            .is_err()
    );
    let available = catalogue.search_available(&all_query()).unwrap();
    assert!(!available.complete);
    assert_eq!(available.result.count, None);
    assert_eq!(available.result.hits.len(), 0);
    resume_tx.send(()).unwrap();

    let first = publication_rx.recv().unwrap();
    assert_eq!(first.0.processed_files, 2_048);
    assert!(!first.initial_construction_complete());
    assert!(
        first
            .search_stream(
                &all_query(),
                crate::streaming::StreamOptions::default(),
                &crate::streaming::CancellationToken::new(),
                &mut |_| Ok(crate::streaming::StreamControl::Continue),
            )
            .is_err()
    );
    let first_results = first.search_available(&all_query()).unwrap();
    assert_eq!(first_results.result.hits.len(), 2_048);
    assert_eq!(first_results.result.count, None);
    assert!(!first_results.result.counters.match_count_complete);
    let filtered = |expression: &str| {
        let mut query = all_query();
        query.filter = Some(parse_filter(expression).unwrap());
        query
    };
    assert_eq!(
        first
            .search_available(&filtered("name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        0
    );
    assert_eq!(
        first
            .search_available(&filtered("name = 'n0' || name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        1
    );
    assert_eq!(
        first
            .search_available(&filtered("!(name = 'n2049')"))
            .unwrap()
            .result
            .hits
            .len(),
        2_048
    );
    let mut follow_query = all_query();
    follow_query.options.count = false;
    let first_delta = first
        .search_added_since(&catalogue, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(first_delta.result.hits.len(), 2_048);
    resume_tx.send(()).unwrap();

    let second = publication_rx.recv().unwrap();
    assert_eq!(second.0.processed_files, 2_050);
    assert!(!second.initial_construction_complete());
    let second_results = second.search_available(&all_query()).unwrap();
    assert_eq!(second_results.result.hits.len(), 2_050);
    assert_eq!(second_results.result.count, None);
    assert_eq!(
        second
            .search_available(&filtered("name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        1
    );
    assert_eq!(
        second
            .search_available(&filtered("name = 'n0' || name = 'n2049'"))
            .unwrap()
            .result
            .hits
            .len(),
        2
    );
    assert_eq!(
        second
            .search_available(&filtered("!(name = 'n2049')"))
            .unwrap()
            .result
            .hits
            .len(),
        2_049
    );
    let second_delta = second
        .search_added_since(&first, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(second_delta.result.hits.len(), 2);
    let coalesced = second
        .search_added_since(&catalogue, &follow_query)
        .unwrap()
        .unwrap();
    assert_eq!(coalesced.result.hits.len(), 2_050);
    resume_tx.send(()).unwrap();

    let finished = worker.join().unwrap().unwrap();
    let complete = finished.view().unwrap();
    assert!(complete.initial_construction_complete());
    let final_results = complete.search(&all_query()).unwrap();
    assert_eq!(final_results.count, Some(2_050));
    assert_eq!(final_results.hits.len(), 2_050);
    let mut metadata = all_query();
    metadata.options.evidence = true;
    metadata.filter = Some(parse_filter("@collection.name = 'data'").unwrap());
    let metadata_result = complete.search(&metadata).unwrap();
    assert_eq!(metadata_result.count, Some(2_050));
    assert_eq!(metadata_result.counters.files_opened, 0);
    metadata.filter = Some(parse_filter("@file.created:null = true").unwrap());
    assert_eq!(complete.search(&metadata).unwrap().counters.files_opened, 0);
    let mut negative = all_query();
    negative.options.evidence = true;
    negative.filter = Some(parse_filter("!(@collection.name = 'other')").unwrap());
    let negative_result = complete.search(&negative).unwrap();
    assert_eq!(negative_result.count, Some(2_050));
    assert_eq!(negative_result.counters.files_opened, 0);
    assert_eq!(finished.status().unwrap().processed_files, 2_050);
    assert_eq!(finished.status().unwrap().build_source_reads, 2_050);
    assert_eq!(finished.status().unwrap().build_parses, 2_050);
    assert_eq!(
        final_results
            .hits
            .iter()
            .map(|hit| hit.record_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2_050
    );
    assert!(!first.initial_construction_complete());
    assert!(first.search(&all_query()).is_err());
    let cache = root.with_extension("idx");
    finished.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(
        &root,
        vec![restore_definition],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    assert_eq!(
        restored.view().unwrap().search(&all_query()).unwrap().count,
        Some(2_050)
    );
    std::fs::remove_file(cache).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_malformed_file_resolves_each_collection_diagnostic() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-shared-diagnostic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/broken.json"), b"{ broken").unwrap();
    let definitions = vec![
        CollectionDefinition::folder("first", "data", SourceFormat::Json),
        CollectionDefinition::folder("second", "data", SourceFormat::Json),
    ];
    let index = Index::start_incremental_build(&root, definitions, RepositoryOptions::default())
        .unwrap()
        .wait_complete()
        .unwrap();
    let view = index.view().unwrap();
    let mut query = all_query();
    query.collections.clear();
    let result = view.search(&query).unwrap();
    assert_eq!(result.hits.len(), 2);
    assert_eq!(index.status().unwrap().fallback_records, 2);
    for hit in result.hits {
        assert_eq!(hit.record_class, crate::inventory::RecordClass::RawFallback);
        let diagnostic = view
            .diagnostic(FileId(hit.file_id), hit.diagnostic_ref)
            .unwrap();
        let expected_collection = view
            .repository()
            .collections()
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
            .unwrap();
        assert_eq!(diagnostic.collection.0 as usize, expected_collection);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jsonl_valid_and_malformed_records_roundtrip_without_duplicate_ids() {
    let root = std::env::temp_dir().join(format!(
        "objekte-initial-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/multi.jsonl"),
        b"{\"name\":\"first\"}\n{broken}\n{\"name\":\"third\"}\n",
    )
    .unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let index = Index::start_incremental_build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let result = view.search(&all_query()).unwrap();
    assert_eq!(result.count, Some(3));
    assert_eq!(result.hits.len(), 3);
    assert_eq!(
        result
            .hits
            .iter()
            .filter(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback)
            .count(),
        1
    );
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| &hit.record_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for hit in &result.hits {
        if hit.record_class == crate::inventory::RecordClass::RawFallback {
            assert!(
                view.diagnostic(FileId(hit.file_id), hit.diagnostic_ref)
                    .is_some()
            );
        }
    }
    let cache = root.with_extension("idx");
    index.flush_cache(&cache).unwrap();
    let restored = Index::load_cache(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    let restored_result = restored.view().unwrap().search(&all_query()).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| (
                &hit.record_id,
                hit.byte_start,
                hit.byte_end,
                hit.record_class
            ))
            .collect::<Vec<_>>(),
        restored_result
            .hits
            .iter()
            .map(|hit| (
                &hit.record_id,
                hit.byte_start,
                hit.byte_end,
                hit.record_class
            ))
            .collect::<Vec<_>>()
    );
    std::fs::remove_file(cache).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
