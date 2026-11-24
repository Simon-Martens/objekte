use super::*;

use crate::index::bigram::BigramAlphabet;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::model::LogicalType;
use crate::model::RecordSlot;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::{FieldDefinition, Schema};
use crate::source::SourceFormat;

#[test]
fn live_posting_pages_and_selections_exclude_retired_slots_and_bind_revision() {
    let root = std::env::temp_dir().join(format!(
        "objekte-live-posting-values-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00", r#"{"tag":"a"}"#),
        ("01", r#"{"tag":"a"}"#),
        ("02", r#"{"tag":"b"}"#),
        ("03", r#"{"tag":"c"}"#),
        ("04", "{"),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["tag".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let all = repo.active.clone();
    let mut live_segments = repo.segments.clone();
    {
        let group = Arc::make_mut(&mut live_segments[0]);
        let segment = Arc::make_mut(&mut group[0]);
        segment.active = segment
            .active
            .retired(&[RecordSlot(1), RecordSlot(3), RecordSlot(4)])
            .unwrap();
    }
    let live = all.clone();
    let old_revision = IndexRevision::INITIAL;
    let new_revision = old_revision.next().unwrap();
    let mut request = PostingValuesRequest::new("data", "tag", 1);
    let old_first = posting_values_segmented(
        &repo,
        &request,
        old_revision,
        &all,
        &repo.segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(old_first.values[0].count, 2);
    assert_eq!(old_first.coverage.unavailable_records, 1);
    let first = posting_values_segmented(
        &repo,
        &request,
        new_revision,
        &live,
        &live_segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(first.values[0].value, ScalarKey::String("a".into()));
    assert_eq!(first.values[0].count, 1);
    assert_eq!(first.coverage.eligible_records, 2);
    assert_eq!(first.coverage.unavailable_records, 0);
    assert_eq!(first.coverage.present_records, 2);
    assert_eq!(first.coverage.indexed_records, 2);
    assert!(first.coverage.value_counts_complete);
    request.continuation = old_first.continuation;
    assert!(
        posting_values_segmented(
            &repo,
            &request,
            new_revision,
            &live,
            &live_segments,
            &repo.catalog,
        )
        .is_err()
    );
    request.continuation = first.continuation;
    let second = posting_values_segmented(
        &repo,
        &request,
        new_revision,
        &live,
        &live_segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(second.values[0].value, ScalarKey::String("b".into()));
    assert_eq!(second.values[0].count, 1);
    assert!(second.continuation.is_none());

    let query = crate::query::Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(crate::query::parse_filter("tag = 'a'").unwrap()),
        options: crate::query::QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: crate::query::options::ReturnMode::Locator,
            evidence: false,
        },
    };
    let old_selection = select_exact_segmented(
        &repo,
        "data",
        &query,
        old_revision,
        &all,
        &repo.segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(old_selection.len(), 2);
    let live_selection = select_exact_segmented(
        &repo,
        "data",
        &query,
        new_revision,
        &live,
        &live_segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(live_selection.len(), 1);
    request.continuation = None;
    request.selection = Some(old_selection);
    assert!(
        posting_values_segmented(
            &repo,
            &request,
            new_revision,
            &live,
            &live_segments,
            &repo.catalog,
        )
        .is_err()
    );
    request.selection = Some(live_selection);
    let selected = posting_values_segmented(
        &repo,
        &request,
        new_revision,
        &live,
        &live_segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(selected.values[0].count, 1);
    assert_eq!(selected.coverage.selected_records, Some(1));
    assert_eq!(
        repo.posting_values(&PostingValuesRequest::new("data", "tag", 10))
            .unwrap()
            .values
            .len(),
        3
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_values_expose_partial_coverage_across_pages_and_selections() {
    let root = std::env::temp_dir().join(format!("objekte-inferred-values-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, content) in [
        ("00", r#"{"code":"A1","flag":true}"#),
        ("01", r#"{"code":"B2","flag":false}"#),
        ("02", r#"{"code":"Long","flag":true}"#),
        ("03", r#"{"code":null}"#),
        ("04", r#"{"flag":false}"#),
        ("05", "{"),
        ("06", r#"{"code":["A1","Long"]}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), content).unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["flag".into()],
            kind: LogicalType::Bool,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            policy: InferencePolicy {
                short_string_max_utf8_bytes: 2,
                ..Default::default()
            },
        },
        ..Default::default()
    };
    let check = |repo: &Repository| {
        let mut request = PostingValuesRequest::new("data", "code", 1);
        let first = repo.posting_values(&request).unwrap();
        assert_eq!(first.values[0].value, ScalarKey::String("A1".into()));
        assert!(first.continuation.is_some());
        assert_eq!(first.values[0].count, 2);
        assert_eq!(first.coverage.eligible_records, 6);
        assert_eq!(first.coverage.unavailable_records, 1);
        assert_eq!(first.coverage.indexed_records, 2);
        assert_eq!(first.coverage.unresolved_records, 4);
        assert!(!first.coverage.value_counts_complete);
        request.continuation = first.continuation;
        let second = repo.posting_values(&request).unwrap();
        assert_eq!(second.values[0].value, ScalarKey::String("B2".into()));
        assert!(second.continuation.is_none());
        assert_eq!(second.coverage, first.coverage);

        let query = |filter: &str| crate::query::Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(crate::query::parse_filter(filter).unwrap()),
            options: crate::query::QueryOptions {
                sort: vec![],
                limit: 10,
                offset: 0,
                count: false,
                return_mode: crate::query::options::ReturnMode::Locator,
                evidence: false,
            },
        };
        request.continuation = None;
        request.selection = Some(repo.select_exact("data", &query("code = 'A1'")).unwrap());
        let short = repo.posting_values(&request).unwrap();
        assert_eq!(short.coverage.indexed_records, 1);
        assert_eq!(short.coverage.unresolved_records, 0);
        assert!(short.coverage.value_counts_complete);
        assert_eq!(short.values[0].count, 1);
        request.selection = Some(repo.select_exact("data", &query("code = 'Long'")).unwrap());
        let long = repo.posting_values(&request).unwrap();
        assert_eq!(long.coverage.indexed_records, 0);
        assert_eq!(long.coverage.unresolved_records, 1);
        assert!(long.values.is_empty());
        assert!(!long.coverage.value_counts_complete);
        request.selection = Some(
            repo.select_exact("data", &query("@record.class = 1"))
                .unwrap(),
        );
        let fallback = repo.posting_values(&request).unwrap();
        assert_eq!(fallback.coverage.unavailable_records, 1);
        assert_eq!(fallback.coverage.eligible_records, 0);
        assert!(fallback.coverage.value_counts_complete);
        let flag = repo
            .posting_values(&PostingValuesRequest::new("data", "flag", 10))
            .unwrap();
        assert!(flag.coverage.value_counts_complete);
        assert_eq!(flag.coverage.unresolved_records, 0);
        let mut unsupported = PostingValuesRequest::new("data", "code", 10);
        unsupported.binding = Some("other".into());
        assert!(repo.posting_values(&unsupported).is_err());
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    check(&repo);
    let restored = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );
    check(&restored);
    use crate::construction::batch::SourceFileInput;
    use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
    use crate::construction::segment_admission::SegmentAdmission;
    use crate::index::segment::PublishedSegment;
    use crate::inventory::SourceRevision;
    use crate::lifecycle::IndexRevision;
    use crate::lifecycle::catalog::FileCatalog;
    use crate::model::{CollectionId, FileId};
    for (name, body) in [
        ("07", r#"{"code":"A1"}"#),
        ("08", r#"{"code":"Long2"}"#),
        ("09", r#"{"code":123}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let state = &restored.collections()[0];
    let mut admission = None;
    for (index, name) in ["07", "08", "09"].into_iter().enumerate() {
        let relative_path = format!("data/{name}.json");
        let path = root.join(&relative_path);
        let prepared = prepare_file_build(
            &root,
            SourceFileInput {
                id: FileId((restored.files().files.len() + index) as u32),
                relative_path: relative_path.into(),
                revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap())
                    .unwrap(),
                created_ns: None,
            },
            &[FilePreparationBinding {
                collection: CollectionId(0),
                definition: state.required_file_definition(),
                source: &state.required_file_reader(),
            }],
            restored.options().bigram_alphabet,
            restored.options().inference,
            1,
        )
        .unwrap();
        let batch = admission.get_or_insert_with(|| {
            SegmentAdmission::new(CollectionId(0), &state.required_file_reader())
        });
        batch
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let index = admission
        .unwrap()
        .finish()
        .into_indexed(
            &state.definition,
            restored.options().bigram_alphabet,
            restored.options().inference,
            1,
            &pool,
        )
        .unwrap();
    let mut catalog = FileCatalog::from_repository(Arc::clone(&restored)).unwrap();
    for file in &index.files {
        catalog = catalog.appended(file.entry.clone(), None, None).unwrap();
    }
    let active = restored.active.clone();
    let mut all_segments = restored.segments.clone();
    let mut first_group = (*all_segments[0]).clone();
    let next_id = first_group
        .iter()
        .map(|segment| segment.id.get())
        .max()
        .unwrap_or(0)
        + 1;
    first_group.push(Arc::new(PublishedSegment {
        id: crate::index::segment::SegmentId::new(next_id),
        active: ActiveRecords::all(index.records.len()).unwrap(),
        index: Arc::new(index),
        observation: None,
    }));
    all_segments[0] = Arc::new(first_group);
    let segments = all_segments;
    let page = posting_values_segmented(
        &restored,
        &PostingValuesRequest::new("data", "code", 10),
        IndexRevision(2),
        &active,
        &segments,
        &catalog,
    )
    .unwrap();
    assert_eq!(page.coverage.eligible_records, 9);
    assert_eq!(page.coverage.indexed_records, 3);
    assert_eq!(page.coverage.unresolved_records, 6);
    assert!(!page.coverage.value_counts_complete);
    assert_eq!(
        page.values
            .iter()
            .find(|entry| { entry.value == ScalarKey::String("A1".into()) })
            .unwrap()
            .count,
        3
    );
    assert!(
        !page
            .values
            .iter()
            .any(|entry| { entry.value == ScalarKey::String("Long2".into()) })
    );
    assert!(
        page.values
            .iter()
            .all(|entry| !matches!(entry.value, ScalarKey::Number(_)))
    );
    let mut request = PostingValuesRequest::new("data", "code", 1);
    let mut paged = Vec::new();
    loop {
        let next = posting_values_segmented(
            &restored,
            &request,
            IndexRevision(2),
            &active,
            &segments,
            &catalog,
        )
        .unwrap();
        assert_eq!(next.coverage, page.coverage);
        paged.extend(next.values);
        let Some(cursor) = next.continuation else {
            break;
        };
        request.continuation = Some(cursor);
    }
    assert_eq!(paged, page.values);
    std::fs::remove_file(root.join("data/08.json")).unwrap();
    assert!(
        posting_values_segmented(
            &restored,
            &PostingValuesRequest::new("data", "code", 10),
            IndexRevision(2),
            &active,
            &segments,
            &catalog,
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_facet_fallback_uses_delta_local_slots_and_record_classes() {
    use crate::construction::batch::SourceFileInput;
    use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
    use crate::construction::segment_admission::SegmentAdmission;
    use crate::index::segment::PublishedSegment;
    use crate::inventory::{ActiveRecords, SourceRevision};
    use crate::lifecycle::IndexRevision;
    use crate::lifecycle::catalog::FileCatalog;
    use crate::model::{CollectionId, FileId, RecordSlot};

    let root = std::env::temp_dir().join(format!(
        "objekte-inferred-fallback-slots-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/base.json"), r#"{"code":1}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["code".into()],
            kind: LogicalType::Unknown,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            policy: InferencePolicy {
                min_nonblank_occurrences: 1,
                min_nonblank_records: 1,
                minimum_repeated_percent: 0,
                ..Default::default()
            },
        },
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let repo = Arc::new(
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            options.clone(),
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap(),
    );

    let second = root.join("data/second.json");
    let third = root.join("data/third.json");
    let retired = root.join("data/retired.json");
    std::fs::write(&second, r#"{"code":2}"#).unwrap();
    std::fs::write(&third, r#"{"code":"mixed"}"#).unwrap();
    std::fs::write(&retired, r#"{"code":4}"#).unwrap();

    let state = &repo.collections()[0];
    let mut admission = None;
    for (index, path) in [&second, &third, &retired].into_iter().enumerate() {
        let relative_path = format!("data/{}", path.file_name().unwrap().to_string_lossy());
        let prepared = prepare_file_build(
            &root,
            SourceFileInput {
                id: FileId((repo.files().files.len() + index) as u32),
                relative_path: relative_path.into(),
                revision: SourceRevision::from_metadata(&std::fs::metadata(path).unwrap()).unwrap(),
                created_ns: None,
            },
            &[FilePreparationBinding {
                collection: CollectionId(0),
                definition: state.required_file_definition(),
                source: &state.required_file_reader(),
            }],
            repo.options().bigram_alphabet,
            repo.options().inference,
            1,
        )
        .unwrap();
        let batch = admission.get_or_insert_with(|| {
            SegmentAdmission::new(CollectionId(0), &state.required_file_reader())
        });
        batch
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let segment = admission
        .unwrap()
        .finish()
        .into_indexed(
            &state.definition,
            repo.options().bigram_alphabet,
            repo.options().inference,
            1,
            &pool,
        )
        .unwrap();
    let active = ActiveRecords::all(segment.records.len())
        .unwrap()
        .retired(&[RecordSlot(2)])
        .unwrap();
    let segment = Arc::new(PublishedSegment {
        id: crate::index::segment::SegmentId::new(1),
        active,
        index: Arc::new(segment),
        observation: None,
    });
    let mut catalog = FileCatalog::from_repository(Arc::clone(&repo)).unwrap();
    for file in &segment.index.files {
        catalog = catalog.appended(file.entry.clone(), None, None).unwrap();
    }
    std::fs::remove_file(&retired).unwrap();
    let mut segments = repo.segments.clone();
    let mut group = (*segments[0]).clone();
    group.push(segment);
    segments[0] = Arc::new(group);
    let request = PostingValuesRequest::new("data", "code", 10);
    let delta = posting_values_segmented(
        &repo,
        &request,
        IndexRevision(2),
        &repo.active,
        &segments,
        &catalog,
    )
    .unwrap();
    let fresh = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap()
    .posting_values(&request)
    .unwrap();
    assert_eq!(delta.values, fresh.values);
    assert_eq!(
        delta.coverage.eligible_records,
        fresh.coverage.eligible_records
    );
    assert_eq!(
        delta.coverage.present_records,
        fresh.coverage.present_records
    );
    assert_eq!(delta.values.iter().map(|entry| entry.count).sum::<u64>(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_inferred_facet_resolution_is_independent_of_segment_order() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segmented-facet-order-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/00_other.json"), r#"{"other":true}"#).unwrap();
    std::fs::write(root.join("data/01_value.json"), r#"{"x":7}"#).unwrap();
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        options.clone(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.active[0].len(), 0);
    let state = &repo.collections()[0];
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut initial_segments = Vec::new();
    for file in &repo.files().files {
        if !file.collections.contains(&crate::model::CollectionId(0)) {
            continue;
        }
        let input = crate::construction::batch::SourceFileInput {
            id: file.id,
            relative_path: file.relative_path.clone(),
            revision: file.revision,
            created_ns: None,
        };
        let prepared = crate::construction::file_preparation::prepare_file_build(
            &root,
            input.clone(),
            &[
                crate::construction::file_preparation::FilePreparationBinding {
                    collection: crate::model::CollectionId(0),
                    definition: state.required_file_definition(),
                    source: &state.required_file_reader(),
                },
            ],
            repo.options().bigram_alphabet,
            repo.options().inference,
            1,
        )
        .unwrap();
        let mut admission = crate::construction::segment_admission::SegmentAdmission::new(
            crate::model::CollectionId(0),
            &state.required_file_reader(),
        );
        admission
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
        let index = admission
            .finish()
            .into_indexed(
                &state.definition,
                repo.options().bigram_alphabet,
                repo.options().inference,
                2,
                &pool,
            )
            .unwrap();
        initial_segments.push(Arc::new(PublishedSegment {
            id: crate::index::segment::SegmentId::new(initial_segments.len() as u64 + 1),
            active: ActiveRecords::all(index.record_count()).unwrap(),
            index: Arc::new(index),
            observation: None,
        }));
    }
    assert_eq!(initial_segments.len(), 2);
    let first = initial_segments[0].clone();
    let second = initial_segments[1].clone();
    assert_ne!(first.id, second.id);
    let forward_segments = vec![Arc::new(vec![first.clone(), second.clone()])];
    let reverse_segments = vec![Arc::new(vec![second, first])];
    let request = PostingValuesRequest::new("data", "x", 10);
    let forward = posting_values_segmented(
        &repo,
        &request,
        repo.revision,
        &repo.active,
        &forward_segments,
        &repo.catalog,
    )
    .unwrap();
    let reverse = posting_values_segmented(
        &repo,
        &request,
        repo.revision,
        &repo.active,
        &reverse_segments,
        &repo.catalog,
    )
    .unwrap();
    assert_eq!(reverse.values, forward.values);
    assert_eq!(reverse.coverage, forward.coverage);
    assert_eq!(reverse.continuation, forward.continuation);
    assert_eq!(
        forward.values,
        [PostingValueCount {
            value: ScalarKey::Number(crate::model::Number::integer(7)),
            count: 1,
        }]
    );
    assert_eq!(
        forward.coverage,
        PostingValuesCoverage {
            eligible_records: 2,
            unavailable_records: 0,
            selected_records: None,
            present_records: 1,
            explicit_null_records: 0,
            indexed_records: 2,
            unresolved_records: 0,
            value_counts_complete: true,
        }
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn finalized_non_value_inference_does_not_authorize_facets() {
    let opt_out_root = std::env::temp_dir().join(format!(
        "objekte-finalized-opt-out-facet-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&opt_out_root);
    std::fs::create_dir_all(opt_out_root.join("data")).unwrap();
    std::fs::write(opt_out_root.join("data/00.json"), r#"{"x":"A"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .prohibit_auto_index(vec!["x".into()])
        .unwrap();
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let opt_out = crate::lifecycle::Index::build_repository_for_test(
        &opt_out_root,
        vec![definition.clone()],
        options.clone(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(
        opt_out.inference_report("data").unwrap().fields[0].index_status,
        crate::inference::InferenceIndexStatus::ExplicitOptOut
    );
    let mut opt_out_segments = opt_out.segments.clone();
    {
        let group = Arc::make_mut(&mut opt_out_segments[0]);
        let published = Arc::make_mut(&mut group[0]);
        let segment = Arc::make_mut(&mut published.index);
        let mut posting = PostingIndex::new(&definition.schema.fields[&vec!["x".into()]]);
        posting
            .add(
                RecordSlot(0),
                &crate::model::Value::from_json(serde_json::json!({"x":"A"})).unwrap(),
            )
            .unwrap();
        posting.finish_stats();
        segment
            .inference
            .as_mut()
            .unwrap()
            .indexes
            .insert(vec!["x".into()], posting);
    }
    assert!(
        posting_values_segmented(
            &opt_out,
            &PostingValuesRequest::new("data", "x", 10),
            opt_out.revision,
            &opt_out.active,
            &opt_out_segments,
            &opt_out.catalog,
        )
        .is_err()
    );
    std::fs::remove_dir_all(opt_out_root).unwrap();

    let length_root = std::env::temp_dir().join(format!(
        "objekte-finalized-length-only-facet-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&length_root);
    std::fs::create_dir_all(length_root.join("data")).unwrap();
    for (name, body) in [
        ("00.json", r#"{"x":[]}"#),
        ("01.json", r#"{"x":[1,2]}"#),
        ("02.json", r#"{"x":[1,2]}"#),
    ] {
        std::fs::write(length_root.join("data").join(name), body).unwrap();
    }
    let length = crate::lifecycle::Index::build_repository_for_test(
        &length_root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    max_distinct_values_per_path: 1,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let diagnostic = length
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|diagnostic| diagnostic.path == ["x"])
        .unwrap();
    assert!(!diagnostic.value_dictionary_complete);
    assert!(!diagnostic.short_string_candidates);
    assert!(
        length
            .posting_values(&PostingValuesRequest::new("data", "x", 10))
            .is_err()
    );
    std::fs::remove_dir_all(length_root).unwrap();
}

#[test]
fn typed_posting_values_count_distinct_records_without_source_reads() {
    let root = std::env::temp_dir().join(format!("objekte-posting-values-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/multi.jsonl");
    std::fs::write(&path, b"{\"tags\":[\"a\",\"a\",\"b\"]}\n{\"tags\":[\"a\",\"c\"]}\n{\"tags\":null}\n{}\n{\"tags\":\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["tags".into()],
            kind: LogicalType::Array(Box::new(LogicalType::String)),
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            ..Default::default()
        };
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let mut request = PostingValuesRequest::new("data", "tags", 2);
        let first = repo.posting_values(&request).unwrap();
        assert_eq!(
            first.values,
            [
                PostingValueCount {
                    value: ScalarKey::String("a".into()),
                    count: 2
                },
                PostingValueCount {
                    value: ScalarKey::String("b".into()),
                    count: 1
                }
            ]
        );
        assert_eq!(
            first.coverage,
            PostingValuesCoverage {
                eligible_records: 4,
                unavailable_records: 1,
                selected_records: None,
                present_records: 3,
                explicit_null_records: 1,
                indexed_records: 4,
                unresolved_records: 0,
                value_counts_complete: true,
            }
        );
        assert_eq!(first.visited_keys, 3);
        request.continuation = first.continuation;
        let second = repo.posting_values(&request).unwrap();
        assert_eq!(
            second.values,
            [PostingValueCount {
                value: ScalarKey::String("c".into()),
                count: 1
            }]
        );
        assert!(second.continuation.is_none());
        request.descending = true;
        request.continuation = None;
        let reverse = repo.posting_values(&request).unwrap();
        assert_eq!(
            reverse
                .values
                .iter()
                .map(|entry| &entry.value)
                .collect::<Vec<_>>(),
            [
                &ScalarKey::String("c".into()),
                &ScalarKey::String("b".into())
            ]
        );
        let selected_query = crate::query::Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(crate::query::parse_filter("tags ?= 'b'").unwrap()),
            options: crate::query::QueryOptions {
                sort: vec![],
                limit: 1,
                offset: 0,
                count: false,
                return_mode: crate::query::options::ReturnMode::Locator,
                evidence: false,
            },
        };
        let selection = repo.select_exact("data", &selected_query).unwrap();
        assert_eq!(selection.len(), 1);
        let parameter_query = crate::query::Query {
            filter: Some(crate::query::parse_filter("tags ?= $chosen_tag").unwrap()),
            ..selected_query.clone()
        };
        assert!(repo.select_exact("data", &parameter_query).is_err());
        let parameter_query = parameter_query
            .with_parameters(&std::collections::HashMap::from([(
                "chosen_tag".into(),
                crate::model::Value::String("a".into()),
            )]))
            .unwrap();
        assert_eq!(
            repo.select_exact("data", &parameter_query).unwrap().len(),
            2
        );
        let text_query = crate::query::Query {
            filter: None,
            fts: Some(crate::query::FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: "a".into(),
                fields: vec![],
            }),
            ..selected_query.clone()
        };
        let text_selection = repo.select_exact("data", &text_query).unwrap();
        assert_eq!(text_selection.len(), 3);
        let mut text_request = PostingValuesRequest::new("data", "tags", 10);
        text_request.selection = Some(text_selection);
        let text_values = repo.posting_values(&text_request).unwrap();
        assert_eq!(text_values.coverage.eligible_records, 2);
        assert_eq!(text_values.coverage.unavailable_records, 1);
        assert_eq!(
            text_values
                .values
                .iter()
                .map(|entry| entry.count)
                .collect::<Vec<_>>(),
            [2, 1, 1]
        );
        let mut selected_request = PostingValuesRequest::new("data", "tags", 1);
        selected_request.selection = Some(selection.clone());
        let first_selected = repo.posting_values(&selected_request).unwrap();
        assert_eq!(
            first_selected.values,
            [PostingValueCount {
                value: ScalarKey::String("a".into()),
                count: 1
            }]
        );
        assert_eq!(
            first_selected.coverage,
            PostingValuesCoverage {
                eligible_records: 1,
                unavailable_records: 0,
                selected_records: Some(1),
                present_records: 1,
                explicit_null_records: 0,
                indexed_records: 1,
                unresolved_records: 0,
                value_counts_complete: true,
            }
        );
        selected_request.continuation = first_selected.continuation;
        let second_selected = repo.posting_values(&selected_request).unwrap();
        assert_eq!(
            second_selected.values,
            [PostingValueCount {
                value: ScalarKey::String("b".into()),
                count: 1
            }]
        );
        assert!(second_selected.continuation.is_none());
        selected_request.selection = Some(repo.select_exact("data", &selected_query).unwrap());
        assert!(repo.posting_values(&selected_request).is_err());
        let fallback_query = crate::query::Query {
            filter: Some(crate::query::parse_filter("@record.class = 1").unwrap()),
            ..selected_query.clone()
        };
        let fallback_selection = repo.select_exact("data", &fallback_query).unwrap();
        assert_eq!(fallback_selection.len(), 1);
        let mut fallback_count = fallback_query.clone();
        fallback_count.options.count = true;
        fallback_count.options.limit = 0;
        fallback_count.options.offset = 3;
        let fallback_count = repo.search(&fallback_count).unwrap();
        assert_eq!(fallback_count.count, Some(1));
        assert_eq!(fallback_count.counters.files_opened, 0);
        selected_request.selection = Some(fallback_selection);
        selected_request.continuation = None;
        assert_eq!(
            repo.posting_values(&selected_request).unwrap().coverage,
            PostingValuesCoverage {
                eligible_records: 0,
                unavailable_records: 1,
                selected_records: Some(1),
                present_records: 0,
                explicit_null_records: 0,
                indexed_records: 0,
                unresolved_records: 0,
                value_counts_complete: true,
            }
        );
        assert!(
            repo.posting_values(&selected_request)
                .unwrap()
                .values
                .is_empty()
        );
        let held = root.join("held.jsonl");
        std::fs::rename(&path, &held).unwrap();
        assert_eq!(repo.select_exact("data", &selected_query).unwrap().len(), 1);
        assert_eq!(
            repo.posting_values(&request).unwrap().values,
            reverse.values
        );
        assert!(
            repo.posting_values(&selected_request)
                .unwrap()
                .values
                .is_empty()
        );
        std::fs::rename(&held, &path).unwrap();
        let restored = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        assert_eq!(
            restored.posting_values(&request).unwrap().values,
            reverse.values
        );
        selected_request.selection = Some(selection);
        assert!(restored.posting_values(&selected_request).is_err());
        request.continuation = reverse.continuation;
        assert!(restored.posting_values(&request).is_err());
        request.continuation = None;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn posting_values_keep_singleton_null_and_empty_traversal_public_counts() {
    let root = std::env::temp_dir().join(format!(
        "objekte-posting-values-legacy-counts-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/records.jsonl"),
        concat!(
            "{\"authors\":[{\"name\":null},{\"name\":\"Ada\"}]}\n",
            "{\"authors\":[]}\n",
            "{}\n",
            "{\"authors\":[{\"name\":null}]}\n",
            "{\"authors\":[{\"name\":null},{\"name\":null}]}\n",
            "{\"authors\":[{\"name\":\"Ada\"}]}\n"
        ),
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["authors".into(), "name".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: vec![true],
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();

    let unrestricted = repo
        .posting_values(&PostingValuesRequest::new("data", "authors.name", 10))
        .unwrap();
    assert_eq!(unrestricted.values.len(), 1);
    assert_eq!(unrestricted.coverage.present_records, 5);
    assert_eq!(unrestricted.coverage.explicit_null_records, 1);

    let query = crate::query::Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(crate::query::parse_filter("authors.name:exists = true").unwrap()),
        options: crate::query::QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: false,
            return_mode: crate::query::options::ReturnMode::Locator,
            evidence: false,
        },
    };
    let selection = repo.select_exact("data", &query).unwrap();
    assert_eq!(selection.len(), 4);
    let mut selected = PostingValuesRequest::new("data", "authors.name", 10);
    selected.selection = Some(selection);
    let selected = repo.posting_values(&selected).unwrap();
    assert_eq!(selected.coverage.present_records, 4);
    assert_eq!(selected.coverage.explicit_null_records, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_terminal_arrays_preserve_legacy_presence_without_source_reads() {
    let root = std::env::temp_dir().join(format!(
        "objekte-inferred-presence-equivalence-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..20 {
        let body = match number {
            0 => r#"{"values":[],"items":[]}"#.to_owned(),
            1 => r#"{"values":[3],"items":[{"value":3}]}"#.to_owned(),
            2 => r#"{"values":[5],"items":[{"value":5}]}"#.to_owned(),
            _ => "{}".to_owned(),
        };
        std::fs::write(root.join(format!("data/{number:02}.json")), body).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    min_nonblank_occurrences: 1,
                    min_nonblank_records: 1,
                    minimum_repeated_percent: 0,
                    ..Default::default()
                },
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let values_request = PostingValuesRequest::new("data", "values", 10);
    let before = repo.posting_values(&values_request).unwrap();
    assert_eq!(before.coverage.present_records, 3);
    assert_eq!(
        before.values.iter().map(|entry| entry.count).sum::<u64>(),
        2
    );
    let query = crate::query::Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(crate::query::parse_filter("items.value:exists = true").unwrap()),
        options: crate::query::QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: false,
            return_mode: crate::query::options::ReturnMode::Locator,
            evidence: false,
        },
    };
    let selection = repo.select_exact("data", &query).unwrap();
    assert_eq!(selection.len(), 2);
    for entry in std::fs::read_dir(root.join("data")).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    assert_eq!(
        repo.posting_values(&values_request).unwrap().values,
        before.values
    );
    assert!(
        repo.posting_values(&PostingValuesRequest::new("data", "items.value", 10))
            .is_err()
    );
    let mut nested_selected = PostingValuesRequest::new("data", "items.value", 10);
    nested_selected.selection = Some(selection);
    let nested = repo.posting_values(&nested_selected).unwrap();
    assert_eq!(nested.coverage.present_records, 2);
    assert_eq!(
        nested.values.iter().map(|entry| entry.count).sum::<u64>(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_facet_requires_data_root_before_inference_resolution() {
    let root = std::env::temp_dir().join(format!("objekte-facet-data-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/00.json"), r#"{"x":7}"#).unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(
        repo.posting_values(&PostingValuesRequest::new("data", "x", 10))
            .unwrap()
            .values,
        [PostingValueCount {
            value: ScalarKey::Number(crate::model::Number::integer(7)),
            count: 1,
        }]
    );
    assert_eq!(
        repo.posting_values(&PostingValuesRequest::new("data", "@backlinks.x", 10))
            .unwrap_err(),
        "posting values require a logical data field"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_named_default_binding_precedes_raw_inference_rejection() {
    use crate::schema::operation::RegisteredBinding;
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!(
        "objekte-facet-named-default-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, title) in [("a.json", "HELLO"), ("b.json", "hello")] {
        std::fs::write(
            root.join("data").join(name),
            serde_json::json!({"title":title}).to_string(),
        )
        .unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .register_operation(
            vec!["title".into()],
            RegisteredBinding {
                name: "fold".into(),
                version: 3,
                operation: Arc::new(FoldPosting { complete: true }),
            },
        )
        .unwrap();
    definition
        .schema
        .set_default_binding(vec!["title".into()], "fold")
        .unwrap();
    definition
        .schema
        .prohibit_auto_index(vec!["title".into()])
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let omitted = repo
        .posting_values(&PostingValuesRequest::new("data", "title", 10))
        .unwrap();
    let mut explicit = PostingValuesRequest::new("data", "title", 10);
    explicit.binding = Some("fold".into());
    let explicit = repo.posting_values(&explicit).unwrap();
    assert_eq!(omitted.values, explicit.values);
    assert_eq!(omitted.coverage, explicit.coverage);
    assert_eq!(
        omitted.values,
        [PostingValueCount {
            value: ScalarKey::String("hello".into()),
            count: 2,
        }]
    );
    let mut raw = PostingValuesRequest::new("data", "title", 10);
    raw.binding = Some("default".into());
    assert!(repo.posting_values(&raw).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_short_string_facet_family_is_independent_of_build_order() {
    let root =
        std::env::temp_dir().join(format!("objekte-facet-mixed-family-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/00_number.json"), r#"{"x":7}"#).unwrap();
    std::fs::write(root.join("data/01_string.json"), r#"{"x":"a"}"#).unwrap();
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let options = RepositoryOptions {
        inference,
        ..Default::default()
    };
    let mut repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let state = &repo.collections()[0];
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut segments = Vec::new();
    for file in &repo.files().files {
        if !file.collections.contains(&crate::model::CollectionId(0)) {
            continue;
        }
        let input = crate::construction::batch::SourceFileInput {
            id: file.id,
            relative_path: file.relative_path.clone(),
            revision: file.revision,
            created_ns: None,
        };
        let prepared = crate::construction::file_preparation::prepare_file_build(
            &root,
            input,
            &[
                crate::construction::file_preparation::FilePreparationBinding {
                    collection: crate::model::CollectionId(0),
                    definition: state.required_file_definition(),
                    source: &state.required_file_reader(),
                },
            ],
            repo.options().bigram_alphabet,
            inference,
            1,
        )
        .unwrap();
        let mut admission = crate::construction::segment_admission::SegmentAdmission::new(
            crate::model::CollectionId(0),
            &state.required_file_reader(),
        );
        admission
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
        let (index, observation) = admission
            .finish()
            .into_indexed_observation_profiled(
                &state.definition,
                repo.options().bigram_alphabet,
                inference,
                2,
                &pool,
                None,
            )
            .unwrap();
        segments.push(Arc::new(PublishedSegment {
            id: crate::index::segment::SegmentId::new(segments.len() as u64 + 1),
            active: ActiveRecords::all(index.record_count()).unwrap(),
            index: Arc::new(index),
            observation: observation.map(Arc::new),
        }));
    }
    assert_eq!(segments.len(), 2);
    assert_eq!(
        segments[0].index.inference.as_ref().unwrap().indexes[&vec!["x".into()]].kind,
        LogicalType::Number
    );
    assert_eq!(
        segments[1].index.inference.as_ref().unwrap().indexes[&vec!["x".into()]].kind,
        LogicalType::String
    );
    let summarize = |ordered: &[Arc<PublishedSegment>]| {
        crate::construction::inference::aggregate_segment_inference(
            inference,
            &repo.collections()[0].definition.schema,
            ordered,
            "data",
        )
        .unwrap()
        .unwrap()
    };
    let forward_summary = summarize(&segments);
    let mut reverse_segments = segments.clone();
    reverse_segments.reverse();
    let reverse_summary = summarize(&reverse_segments);
    for summary in [&forward_summary, &reverse_summary] {
        let field = summary
            .report
            .fields
            .iter()
            .find(|field| field.path == ["x"])
            .unwrap();
        assert!(field.observed_kinds.contains(&LogicalType::Number));
        assert!(field.observed_kinds.contains(&LogicalType::String));
        let diagnostic = summary
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.path == ["x"])
            .unwrap();
        assert!(!diagnostic.value_dictionary_complete);
        assert!(diagnostic.short_string_candidates);
    }
    let expected_coverage = PostingValuesCoverage {
        eligible_records: 2,
        unavailable_records: 0,
        selected_records: None,
        present_records: 2,
        explicit_null_records: 0,
        indexed_records: 1,
        unresolved_records: 1,
        value_counts_complete: false,
    };
    for (summary, ordered) in [
        (forward_summary, segments.clone()),
        (reverse_summary, reverse_segments),
    ] {
        repo.collections[0].global_inference = Some(summary);
        repo.segments = vec![Arc::new(ordered)];
        let page = repo
            .posting_values(&PostingValuesRequest::new("data", "x", 10))
            .unwrap();
        assert_eq!(
            page.values,
            [PostingValueCount {
                value: ScalarKey::String("a".into()),
                count: 1,
            }]
        );
        assert_eq!(page.coverage, expected_coverage);
        assert!(page.continuation.is_none());
        let selection_query = crate::query::Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(crate::query::parse_filter("x = 7").unwrap()),
            options: crate::query::QueryOptions {
                sort: Vec::new(),
                limit: 10,
                offset: 0,
                count: false,
                return_mode: crate::query::options::ReturnMode::Locator,
                evidence: false,
            },
        };
        let mut selected = PostingValuesRequest::new("data", "x", 10);
        selected.selection = Some(repo.select_exact("data", &selection_query).unwrap());
        let selected = repo.posting_values(&selected).unwrap();
        assert!(selected.values.is_empty());
        assert_eq!(selected.coverage.eligible_records, 1);
        assert_eq!(selected.coverage.present_records, 1);
        assert_eq!(selected.coverage.indexed_records, 0);
        assert_eq!(selected.coverage.unresolved_records, 1);
        assert!(!selected.coverage.value_counts_complete);
    }

    let mut structural_index = (*segments[0].index).clone();
    let structural_posting = structural_index
        .inference
        .as_mut()
        .unwrap()
        .indexes
        .get_mut(&vec!["x".into()])
        .unwrap();
    structural_posting.value_complete = false;
    structural_posting.values.clear();
    structural_posting.length_complete = true;
    let structural = Arc::new(PublishedSegment {
        id: crate::index::segment::SegmentId::new(20),
        active: segments[0].active.clone(),
        index: Arc::new(structural_index),
        observation: None,
    });
    repo.collections[0].global_inference = None;
    repo.segments = vec![Arc::new(vec![structural, segments[1].clone()])];
    let local = repo
        .posting_values(&PostingValuesRequest::new("data", "x", 10))
        .unwrap();
    assert_eq!(
        local.values,
        [PostingValueCount {
            value: ScalarKey::String("a".into()),
            count: 1,
        }]
    );
    assert_eq!(local.coverage, expected_coverage);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn numeric_and_temporal_keys_keep_their_types_and_order() {
    use crate::model::Number;
    let root = std::env::temp_dir().join(format!("objekte-posting-typed-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("a", r#"{"amount":1,"published":"2026-01-02"}"#),
        ("b", r#"{"amount":1.5,"published":"2026-01-01"}"#),
        (
            "c",
            r#"{"amount":100000000000000000000,"published":"2026-01-02"}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind) in [
        ("amount", LogicalType::Number),
        ("published", LogicalType::Date),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let numbers = repo
        .posting_values(&PostingValuesRequest::new("data", "amount", 10))
        .unwrap();
    assert_eq!(
        numbers.values,
        [
            PostingValueCount {
                value: ScalarKey::Number(Number::integer(1)),
                count: 1
            },
            PostingValueCount {
                value: ScalarKey::Number(Number::float(1.5).unwrap()),
                count: 1
            },
            PostingValueCount {
                value: ScalarKey::Number(Number::integer(100000000000000000000)),
                count: 1
            },
        ]
    );
    let dates = repo
        .posting_values(&PostingValuesRequest::new("data", "published", 10))
        .unwrap();
    assert_eq!(dates.values.len(), 2);
    assert!(matches!(dates.values[0].value, ScalarKey::Date(_)));
    assert_eq!((dates.values[0].count, dates.values[1].count), (1, 2));
    let mut narrowed = PostingValuesRequest::new("data", "amount", 10);
    narrowed.lower = Some(PostingValueBound {
        value: ScalarKey::Number(Number::integer(1)),
        inclusive: false,
    });
    narrowed.upper = Some(PostingValueBound {
        value: ScalarKey::Number(Number::integer(2)),
        inclusive: true,
    });
    assert_eq!(
        repo.posting_values(&narrowed).unwrap().values,
        [PostingValueCount {
            value: ScalarKey::Number(Number::float(1.5).unwrap()),
            count: 1
        }]
    );
    narrowed.prefix = Some("1".into());
    assert!(repo.posting_values(&narrowed).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Debug)]
struct FoldPosting {
    complete: bool,
}

impl crate::schema::operation::BindingOperation for FoldPosting {
    fn accepts(&self, kind: &LogicalType, operator: crate::query::Operator, _: bool) -> bool {
        *kind == LogicalType::String && operator == crate::query::Operator::Eq
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _: &LogicalType,
    ) -> Result<crate::model::Value, String> {
        match literal {
            crate::query::Literal::String(text) => {
                Ok(crate::model::Value::String(text.to_ascii_lowercase()))
            }
            _ => Err("text required".into()),
        }
    }

    fn map_value(&self, value: &crate::model::Value) -> Option<crate::model::Value> {
        match value {
            crate::model::Value::String(text) => {
                Some(crate::model::Value::String(text.to_ascii_lowercase()))
            }
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.to_ascii_lowercase()
    }

    fn prepare_posting_prefix(&self, prefix: &str) -> Result<Option<String>, String> {
        Ok(Some(prefix.to_ascii_lowercase()))
    }

    fn posting(&self) -> bool {
        true
    }

    fn complete_coverage(&self) -> bool {
        self.complete
    }
}

#[test]
fn named_default_posting_returns_stored_keys_and_rejects_incomplete_coverage() {
    use crate::schema::operation::RegisteredBinding;
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("objekte-posting-binding-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, title) in [("a", "HELLO"), ("b", "hello"), ("c", "World")] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            format!(r#"{{"title":"{title}"}}"#),
        )
        .unwrap();
    }
    for complete in [true, false] {
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
        schema
            .register_operation(
                vec!["title".into()],
                RegisteredBinding {
                    name: "fold".into(),
                    version: 3,
                    operation: Arc::new(FoldPosting { complete }),
                },
            )
            .unwrap();
        schema
            .set_default_binding(vec!["title".into()], "fold")
            .unwrap();
        let definition = CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        };
        let options = RepositoryOptions {
            ..Default::default()
        };
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let request = PostingValuesRequest::new("data", "title", 10);
        if complete {
            assert_eq!(
                repo.posting_values(&request).unwrap().values,
                [
                    PostingValueCount {
                        value: ScalarKey::String("hello".into()),
                        count: 2
                    },
                    PostingValueCount {
                        value: ScalarKey::String("world".into()),
                        count: 1
                    }
                ]
            );
            let mut prefixed = request.clone();
            prefixed.prefix = Some("HE".into());
            assert_eq!(
                repo.posting_values(&prefixed).unwrap().values,
                [PostingValueCount {
                    value: ScalarKey::String("hello".into()),
                    count: 2
                }]
            );
            let mut paged = PostingValuesRequest::new("data", "title", 1);
            let first = repo.posting_values(&paged).unwrap();
            paged.continuation = first.continuation.clone();
            let mut changed_binding = paged.clone();
            changed_binding.binding = Some("default".into());
            assert!(
                repo.posting_values(&changed_binding)
                    .unwrap_err()
                    .contains("continuation scope")
            );

            let restored = crate::lifecycle::Index::start_build(
                &root,
                vec![definition.clone()],
                options,
                crate::source::registry::SourceRegistry::builtin(),
            )
            .and_then(|task| task.wait_complete())
            .unwrap();
            let restored = restored.view().unwrap();
            assert_eq!(
                restored.posting_values(&request).unwrap().values,
                repo.posting_values(&request).unwrap().values
            );
            assert!(
                restored
                    .posting_values(&paged)
                    .unwrap_err()
                    .contains("continuation scope")
            );
        } else {
            assert!(
                repo.posting_values(&request)
                    .unwrap_err()
                    .contains("incomplete")
            );
        }
        let mut explicit = request;
        explicit.binding = Some("default".into());
        assert_eq!(repo.posting_values(&explicit).unwrap().values.len(), 3);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_prefix_ranges_and_continuations_visit_only_relevant_keys() {
    let root = std::env::temp_dir().join(format!("objekte-posting-bounded-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut lines = String::new();
    for number in 0..1000 {
        lines.push_str(&format!(r#"{{"key":"k{number:04}"}}"#));
        lines.push('\n');
    }
    for value in ["\u{10ffff}", "\u{10ffff}x"] {
        lines.push_str(&serde_json::json!({"key":value}).to_string());
        lines.push('\n');
    }
    std::fs::write(root.join("data/values.jsonl"), lines).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["key".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let mut request = PostingValuesRequest::new("data", "key", 3);
    request.prefix = Some("k099".into());
    let first = repo.posting_values(&request).unwrap();
    assert_eq!(
        first
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        ["k0990", "k0991", "k0992"].map(|key| ScalarKey::String(key.into()))
    );
    assert_eq!(first.visited_keys, 4);
    let cursor = first.continuation.clone().unwrap();
    let mut changed = request.clone();
    changed.prefix = Some("k098".into());
    changed.continuation = Some(cursor.clone());
    assert!(repo.posting_values(&changed).is_err());
    changed.prefix = request.prefix.clone();
    changed.limit = 0;
    changed.descending = true;
    assert!(repo.posting_values(&changed).is_err());
    changed.descending = false;
    changed.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0991".into()),
        inclusive: true,
    });
    assert!(repo.posting_values(&changed).is_err());
    request.continuation = Some(cursor);
    let mut collected = first
        .values
        .into_iter()
        .map(|entry| entry.value)
        .collect::<Vec<_>>();
    loop {
        let page = repo.posting_values(&request).unwrap();
        assert!(page.visited_keys <= 4);
        collected.extend(page.values.into_iter().map(|entry| entry.value));
        match page.continuation {
            Some(next) => request.continuation = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected,
        (990..1000)
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    request.continuation = None;
    request.prefix = None;
    request.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0500".into()),
        inclusive: true,
    });
    request.upper = Some(PostingValueBound {
        value: ScalarKey::String("k0503".into()),
        inclusive: false,
    });
    let range = repo.posting_values(&request).unwrap();
    assert_eq!(
        range
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        (500..503)
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    assert_eq!(range.visited_keys, 3);
    request.descending = true;
    let descending = repo.posting_values(&request).unwrap();
    assert_eq!(
        descending
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        (500..503)
            .rev()
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    request.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0501".into()),
        inclusive: true,
    });
    request.upper = Some(PostingValueBound {
        value: ScalarKey::String("k0501".into()),
        inclusive: true,
    });
    assert_eq!(repo.posting_values(&request).unwrap().values.len(), 1);
    request.upper.as_mut().unwrap().inclusive = false;
    assert!(repo.posting_values(&request).unwrap().values.is_empty());
    request.lower = None;
    request.upper = None;
    request.descending = false;
    request.prefix = Some("\u{10ffff}".into());
    let unicode = repo.posting_values(&request).unwrap();
    assert_eq!(
        unicode
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        [
            ScalarKey::String("\u{10ffff}".into()),
            ScalarKey::String("\u{10ffff}x".into())
        ]
    );
    request.prefix = Some(String::new());
    assert_eq!(
        repo.posting_values(&request).unwrap().values[0].value,
        ScalarKey::String("k0000".into())
    );
    request.lower = Some(PostingValueBound {
        value: ScalarKey::Number(crate::model::Number::integer(1)),
        inclusive: true,
    });
    assert!(repo.posting_values(&request).is_err());
    request.lower = None;
    request.limit = MAX_POSTING_VALUES_PAGE + 1;
    assert!(repo.posting_values(&request).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
