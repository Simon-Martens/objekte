use super::*;
use crate::construction::batch::{SourceFileInput, process_batch};
use crate::construction::file_preparation::{
    FilePreparationBinding, PreparedFileBuild, prepare_file_build,
};
use crate::index::bigram::BigramAlphabet;
use crate::index::posting::ScalarKey;
use crate::inference::InferenceConfig;
use crate::inference::{InferenceMode, InferencePolicy};
use crate::inventory::FileStatus;
use crate::inventory::SourceRevision;
use crate::model::{CollectionId, FileId};
use crate::model::{FieldPath, LogicalType, Number};
use crate::repository::configuration::CollectionDefinition;
use crate::schema::FieldDefinition;
use std::time::Instant;

#[test]
fn admits_one_worker_batch_spanning_files_without_reprojecting_postings() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-batch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["n".into()],
            kind: LogicalType::Number,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let mut inputs = Vec::new();
    for (number, bytes) in [b"(\"n\"=1)\n".as_slice(), b"(bad)\n".as_slice()]
        .into_iter()
        .enumerate()
    {
        let name = format!("{number}.rows");
        std::fs::write(root.join(&name), bytes).unwrap();
        inputs.push(SourceFileInput {
            id: FileId(number as u32),
            relative_path: name.clone().into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(root.join(name)).unwrap())
                .unwrap(),
            created_ns: None,
        });
    }
    let batch = process_batch(
        &root,
        &definition,
        CollectionId(0),
        &source,
        &[FileId(0), FileId(1)],
        &inputs,
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        1,
    )
    .unwrap();
    assert_eq!(batch.files.len(), 2);
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    admission.admit_batch(batch).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = admission
        .finish()
        .into_indexed(
            &definition,
            BigramAlphabet::Ascii,
            InferenceConfig::default(),
            1,
            &pool,
        )
        .unwrap();
    assert_eq!(indexed.files.len(), 2);
    assert_eq!(indexed.records.len(), 2);
    assert_eq!(indexed.files[0].records, [RecordSlot(0)]);
    assert_eq!(indexed.files[1].records, [RecordSlot(1)]);
    assert_eq!(indexed.files[1].entry.diagnostics.len(), 1);
    indexed.raw.validate(2).unwrap();
    indexed.postings.validate_eligible(2, 1).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn disabled_inference_does_not_create_compact_observation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-disabled-observation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    std::fs::write(root.join("0.rows"), "(\"x\"=1)\n").unwrap();
    let input = SourceFileInput {
        id: FileId(0),
        relative_path: "0.rows".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(root.join("0.rows")).unwrap())
            .unwrap(),
        created_ns: None,
    };
    let batch = process_batch(
        &root,
        &definition,
        CollectionId(0),
        &source,
        &[input.id],
        std::slice::from_ref(&input),
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    admission.admit_batch(batch).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let (index, observation) = admission
        .finish()
        .into_indexed_observation_profiled(
            &definition,
            BigramAlphabet::Ascii,
            InferenceConfig::default(),
            1,
            &pool,
            None,
        )
        .unwrap();
    assert!(observation.is_none());
    assert!(index.inference.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn transfers_pristine_declared_postings_across_first_and_zero_record_batches() {
    let root = std::env::temp_dir().join(format!(
        "objekte-posting-transfer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    for (name, kind, indexed) in [
        ("n", LogicalType::Number, true),
        ("s", LogicalType::String, false),
        (
            "blocked",
            LogicalType::Array(Box::new(LogicalType::Number)),
            false,
        ),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: true,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
        .schema
        .prohibit_auto_index(vec!["blocked".into()])
        .unwrap();
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    for (name, body) in [
        ("0.rows", ""),
        ("1.rows", "(\"n\"=null,\"blocked\"=[])\n"),
        ("2.rows", "(\"n\"=2,\"s\"=\"value\")\n"),
    ] {
        std::fs::write(root.join(name), body).unwrap();
    }
    let inputs = (0..3)
        .map(|number| {
            let name = format!("{number}.rows");
            SourceFileInput {
                id: FileId(number),
                relative_path: name.clone().into(),
                revision: SourceRevision::from_metadata(
                    &std::fs::metadata(root.join(format!("{number}.rows"))).unwrap(),
                )
                .unwrap(),
                created_ns: None,
            }
        })
        .collect::<Vec<_>>();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let build = |profile: Option<&mut crate::repository::statistics::IncrementalSegmentProfile>| {
        let mut admission = SegmentAdmission::new(CollectionId(0), &source);
        for input in &inputs {
            let batch = process_batch(
                &root,
                &definition,
                CollectionId(0),
                &source,
                &[input.id],
                &inputs,
                BigramAlphabet::Ascii,
                InferenceConfig::default(),
                1,
            )
            .unwrap();
            admission.admit_batch(batch).unwrap();
        }
        admission
            .finish()
            .into_indexed_profiled(
                &definition,
                BigramAlphabet::Ascii,
                InferenceConfig::default(),
                1,
                &pool,
                profile,
            )
            .unwrap()
    };
    let without_profile = build(None);
    let mut profile = crate::repository::statistics::IncrementalSegmentProfile::default();
    let indexed = build(Some(&mut profile));
    assert_eq!(profile.zero_base_posting_transfers, 2);
    assert_eq!(profile.transferred_posting_keys, 7);
    assert_eq!(profile.transferred_posting_members, 3);
    assert_eq!(
        indexed.postings.fields.len(),
        without_profile.postings.fields.len()
    );
    for (path, expected) in &without_profile.postings.fields {
        let actual = &indexed.postings.fields[path];
        assert_eq!(actual.kind, expected.kind, "{path:?} kind");
        assert_eq!(actual.multi, expected.multi, "{path:?} multiplicity");
        assert_eq!(actual.presence, expected.presence, "{path:?} presence");
        assert_eq!(
            actual.null_posting(),
            expected.null_posting(),
            "{path:?} null"
        );
        assert_eq!(actual.lengths, expected.lengths, "{path:?} lengths");
        assert_eq!(actual.values, expected.values, "{path:?} values");
        assert_eq!(actual.summary, expected.summary, "{path:?} summary");
        assert_eq!(
            actual.covered_records, expected.covered_records,
            "{path:?} coverage"
        );
        assert_eq!(
            actual.values_indexed, expected.values_indexed,
            "{path:?} value indexing"
        );
        assert_eq!(
            actual.value_complete, expected.value_complete,
            "{path:?} value completeness"
        );
        assert_eq!(
            actual.length_complete, expected.length_complete,
            "{path:?} length completeness"
        );
        assert_eq!(
            actual.empty_string_complete, expected.empty_string_complete,
            "{path:?} empty-string completeness"
        );
        assert_eq!(
            format!("{:?}", actual.stats),
            format!("{:?}", expected.stats),
            "{path:?} stats"
        );
    }
    assert_eq!(indexed.records.len(), 2);
    assert_eq!(
        indexed.postings.fields[&vec!["n".into()]]
            .null_posting()
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
    assert_eq!(
        indexed.postings.fields[&vec!["blocked".into()]]
            .presence
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
    indexed.postings.validate_eligible(2, 2).unwrap();
    without_profile.postings.validate_eligible(2, 2).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn admits_multiple_files_and_chunks_with_sparse_file_ids() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition =
        CollectionDefinition::folder("data", ".", crate::source::test_support::line_format());
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["n".into()],
            kind: LogicalType::Number,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let binding = FilePreparationBinding {
        factory: crate::index::storage::test_factory(),
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    let mut limited = SegmentAdmission::with_limits(
        CollectionId(0),
        &source,
        crate::index::segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(16_384).unwrap(),
            max_files: std::num::NonZeroUsize::new(1).unwrap(),
        },
    );
    for (id, name, bytes) in [
        (
            FileId(400),
            "one.rows",
            b"(\"n\"=1)\n(\"n\"=2)\n".as_slice(),
        ),
        (FileId(900), "zero.rows", b"".as_slice()),
        (FileId(1200), "broken.rows", b"(bad)\n".as_slice()),
    ] {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id,
            relative_path: name.into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
        let prepared = prepare_file_build(
            &root,
            input,
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        if id == FileId(400) {
            let duplicate = prepare_file_build(
                &root,
                prepared.source.clone(),
                &[binding],
                BigramAlphabet::Ascii,
                inference,
                1,
            )
            .unwrap();
            limited
                .admit(
                    &duplicate.source,
                    duplicate.collections.into_iter().next().unwrap(),
                )
                .unwrap();
        } else if id == FileId(900) {
            assert!(!limited.can_admit(&prepared.source, &prepared.collections[0]));
        }
        let PreparedFileBuild {
            source,
            collections,
            ..
        } = prepared;
        admission
            .admit(&source, collections.into_iter().next().unwrap())
            .unwrap();
    }
    assert_eq!(admission.file_count(), 3);
    assert_eq!(admission.record_count(), 3);
    let segment = admission.finish();
    assert_eq!(
        segment
            .files
            .iter()
            .map(|file| file.entry.id)
            .collect::<Vec<_>>(),
        [FileId(400), FileId(900), FileId(1200)]
    );
    assert_eq!(
        segment
            .files
            .iter()
            .map(|file| file.records.len())
            .collect::<Vec<_>>(),
        [2, 0, 1]
    );
    assert_eq!(
        segment
            .records
            .iter()
            .map(|record| record.slot.0)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(
        segment
            .records
            .iter()
            .map(|record| segment.backing.file_locator(record.slot).unwrap().file)
            .collect::<Vec<_>>(),
        [FileId(400), FileId(400), FileId(1200)]
    );
    assert_eq!(segment.files[2].entry.status, FileStatus::Broken);
    assert_eq!(segment.files[2].entry.diagnostics.len(), 1);
    assert_eq!(segment.records[2].diagnostic_ref, DiagnosticRef(1));
    assert_eq!(segment.batches.len(), 3);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = segment
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    assert_eq!(indexed.records.len(), 3);
    assert_eq!(indexed.logical_records.iter().collect::<Vec<_>>(), [0, 1]);
    assert_eq!(
        indexed
            .fallback_text
            .candidates("bad")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(indexed.id_lookup.len(), 3);
    assert_eq!(
        indexed.postings.fields[&vec!["n".to_owned()]].values
            [&ScalarKey::Number(Number::integer(2))]
            .iter()
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(
        indexed.sorting.columns[&FieldPath::parse("n").unwrap()]
            .ascending
            .iter()
            .map(|slot| slot.0)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(indexed.file_records(FileId(900)).unwrap(), []);
    assert_eq!(
        indexed.file_record(RecordSlot(2)).unwrap().file,
        FileId(1200)
    );
    assert!(indexed.relations.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_physical_file_can_enter_two_collection_segments() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-shared-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("shared.rows");
    std::fs::write(&path, b"(\"n\"=1)\n").unwrap();
    let definitions = [
        CollectionDefinition::folder("left", ".", crate::source::test_support::line_format()),
        CollectionDefinition::folder("right", ".", crate::source::test_support::line_format()),
    ];
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let bindings = [
        FilePreparationBinding {
            factory: crate::index::storage::test_factory(),
            collection: CollectionId(0),
            definition: &definitions[0],
            source: &source,
        },
        FilePreparationBinding {
            factory: crate::index::storage::test_factory(),
            collection: CollectionId(1),
            definition: &definitions[1],
            source: &source,
        },
    ];
    let input = SourceFileInput {
        id: FileId(700),
        relative_path: "shared.rows".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let prepared =
        prepare_file_build(&root, input, &bindings, BigramAlphabet::Ascii, inference, 1).unwrap();
    let mut segments = Vec::new();
    for collection in prepared.collections {
        let mut admission = SegmentAdmission::new(collection.collection, &source);
        admission.admit(&prepared.source, collection).unwrap();
        segments.push(admission.finish());
    }
    assert_eq!(segments.len(), 2);
    assert!(
        segments
            .iter()
            .all(|segment| segment.files[0].entry.id == FileId(700) && segment.records.len() == 1)
    );
    assert_eq!(segments[0].collection, CollectionId(0));
    assert_eq!(segments[1].collection, CollectionId(1));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for (index, segment) in segments.into_iter().enumerate() {
        let indexed = segment
            .into_indexed(
                &definitions[index],
                BigramAlphabet::Ascii,
                inference,
                2,
                &pool,
            )
            .unwrap();
        assert_eq!(indexed.records.len(), 1);
        assert_eq!(indexed.files[0].entry.id, FileId(700));
        assert_eq!(indexed.logical_records.iter().collect::<Vec<_>>(), [0]);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "manual small-file prepare/admit/finalize timing"]
fn measure_single_file_segment_cost() {
    let root = std::env::temp_dir().join(format!("objekte-segment-timing-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("sample.rows");
    std::fs::write(&path, b"(\"name\"=\"a small record\",\"n\"=42)\n").unwrap();
    let input = SourceFileInput {
        id: FileId(500),
        relative_path: "sample.rows".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let definition =
        CollectionDefinition::folder("sample", ".", crate::source::test_support::line_format());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&crate::source::test_support::line_format())
        .unwrap();
    let binding = FilePreparationBinding {
        factory: crate::index::storage::test_factory(),
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let started = Instant::now();
    for _ in 0..100 {
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        let mut admission = SegmentAdmission::new(CollectionId(0), &source);
        admission
            .admit(
                &prepared.source,
                prepared.collections.into_iter().next().unwrap(),
            )
            .unwrap();
        let indexed = admission
            .finish()
            .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
            .unwrap();
        assert_eq!(indexed.records.len(), 1);
    }
    eprintln!(
        "small-file prepare/admit/finalize: {:.1} µs/call (100 calls, debug/release as invoked)",
        started.elapsed().as_secs_f64() * 10_000.0
    );
    std::fs::remove_dir_all(root).unwrap();
}
