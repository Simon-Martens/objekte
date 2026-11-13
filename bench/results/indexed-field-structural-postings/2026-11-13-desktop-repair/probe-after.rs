// Temporary measurement probe for the authorized desktop repair comparison.
#[test]
#[ignore]
fn desktop_delta_benchmark_probe() {
    use objekte_search_core::lifecycle::Index;
    use objekte_search_core::lifecycle_caller::IndexedRepository;
    use objekte_search_core::repository::{CollectionDefinition, Payload, Repository, RepositoryOptions};
    use objekte_search_core::source::SourceFormat;
    let records: usize = std::env::var("OBJEKTE_DELTA_BENCH_RECORDS").unwrap().parse().unwrap();
    let loaded: usize = std::env::var("OBJEKTE_DELTA_BENCH_LOADED").unwrap().parse().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let repository = Repository::open_with_options(root.path(), vec![definition], RepositoryOptions::default()).unwrap();
    let index = Index::from_repository(repository).unwrap();
    let previous = index.view().unwrap();
    let body = "z".repeat(512);
    let mut contents = String::new();
    for number in 0..records {
        contents.push_str(&format!(r#"{{"name":"n{number}","body":"{body}"}}"#));
        contents.push('\n');
    }
    let path = root.path().join("data/bulk.jsonl");
    std::fs::write(&path, contents).unwrap();
    index.add_file(&path).unwrap();
    let view = index.view().unwrap();
    let mut session = crate::backend::test_support::fixture().session;
    session.repository = std::sync::Arc::new(IndexedRepository::new(index.clone()).unwrap());
    let spec = backend::QuerySpec::default();
    let token = CancellationToken::new();
    let query = backend::build_query(&spec).unwrap();
    assert_eq!(query.options.return_mode, objekte_search_core::workload::ReturnMode::Bytes);
    assert!(query.options.evidence);
    let construction_before = index.status().unwrap();
    let started = Instant::now();
    let (_, page) = incremental_delta_page(&session, &previous, &view, &spec, loaded, &token).unwrap();
    let materialized_hits = page.counters.selected_payload_reads as usize;
    let materialized_bytes = page.counters.selected_payload_bytes as usize;
    let residual_source_opens = page.counters.files_opened;
    let peak_retained_payload_bytes = page.counters.peak_retained_payload_bytes as usize;
    let returned_hits = page.hits.len();
    assert_eq!(materialized_hits, returned_hits);
    assert_eq!(materialized_bytes, page.hits.iter().map(|hit| match &hit.payload { Some(Payload::Bytes(bytes)) => bytes.len(), _ => 0 }).sum::<usize>());
    let elapsed_ns = started.elapsed().as_nanos();
    let construction_after = index.status().unwrap();
    assert_eq!(construction_before.build_source_reads, construction_after.build_source_reads);
    assert_eq!(construction_before.build_parses, construction_after.build_parses);
    println!("DELTA_BENCH_JSON:{}", serde_json::json!({
        "records": records,
        "loaded": loaded,
        "elapsed_ns": elapsed_ns,
        "materialized_hits": materialized_hits,
        "materialized_bytes": materialized_bytes,
        "returned_hits": returned_hits,
        "residual_source_opens": residual_source_opens,
        "peak_retained_payload_bytes": peak_retained_payload_bytes,
        "build_source_reads": construction_after.build_source_reads,
        "build_parses": construction_after.build_parses
    }));
}
