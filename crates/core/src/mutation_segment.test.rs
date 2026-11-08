use super::*;
use crate::inference::{InferenceMode, InferencePolicy};
use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;
use crate::reference::FieldDefinition;
use crate::source::{SourceFormat, SourceRegistry};

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
    let mut definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
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
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
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
        SegmentLimits {
            files: 1,
            records: 16_384,
            source_bytes: 64 * 1024 * 1024,
        },
    );
    for (id, name, bytes) in [
        (
            FileId(400),
            "one.jsonl",
            b"{\"n\":1}\n{\"n\":2}\n".as_slice(),
        ),
        (FileId(900), "zero.jsonl", b"".as_slice()),
        (FileId(1200), "broken.jsonl", b"{bad}\n".as_slice()),
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
    let segment = admission.seal();
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
            .map(|record| record.file)
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
    assert_eq!(indexed.record(RecordSlot(2)).unwrap().file, FileId(1200));
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
    let path = root.join("shared.jsonl");
    std::fs::write(&path, b"{\"n\":1}\n").unwrap();
    let definitions = [
        CollectionDefinition::folder("left", ".", SourceFormat::JsonLines),
        CollectionDefinition::folder("right", ".", SourceFormat::JsonLines),
    ];
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let bindings = [
        FilePreparationBinding {
            collection: CollectionId(0),
            definition: &definitions[0],
            source: &source,
        },
        FilePreparationBinding {
            collection: CollectionId(1),
            definition: &definitions[1],
            source: &source,
        },
    ];
    let input = SourceFileInput {
        id: FileId(700),
        relative_path: "shared.jsonl".into(),
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
        segments.push(admission.seal());
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
    let path = root.join("sample.jsonl");
    std::fs::write(&path, b"{\"name\":\"a small record\",\"n\":42}\n").unwrap();
    let input = SourceFileInput {
        id: FileId(500),
        relative_path: "sample.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let definition = CollectionDefinition::folder("sample", ".", SourceFormat::JsonLines);
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
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
            .seal()
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
