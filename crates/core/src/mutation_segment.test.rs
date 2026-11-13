use super::*;
use crate::inference::{InferenceMode, InferencePolicy};
use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::source::{SourceFormat, SourceRegistry};

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
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let mut inputs = Vec::new();
    for (number, bytes) in [b"{\"n\":1}\n".as_slice(), b"{bad}\n".as_slice()]
        .into_iter()
        .enumerate()
    {
        let name = format!("{number}.jsonl");
        std::fs::write(root.join(&name), bytes).unwrap();
        inputs.push(SourceFileInput {
            id: FileId(number as u32),
            relative_path: name.clone().into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(root.join(name)).unwrap())
                .unwrap(),
            created_ns: None,
        });
    }
    let batch = super::super::process_batch(
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
        .seal()
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

#[test]
fn consolidation_matches_direct_admission_without_source_files() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-consolidation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    for (path, kind, indexed) in [
        ("n", LogicalType::Number, true),
        ("ref", LogicalType::Id, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: false,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "target".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
        .sort_fields
        .push(FieldPath::parse("ref").unwrap());
    definition
        .schema
        .register_composite_sort(vec![
            crate::workload::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::workload::SortField {
                field: "ref".into(),
                descending: true,
                expression: None,
            },
        ])
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let files = [
        (
            FileId(100),
            "a.jsonl",
            b"{\"n\":1,\"extra\":11,\"ref\":\"x\"}\n{\"n\":2,\"extra\":22,\"ref\":\"y\"}\n"
                .as_slice(),
        ),
        (
            FileId(500),
            "b.jsonl",
            b"{\"n\":3,\"extra\":33,\"ref\":\"z\"}\n{bad}\n".as_slice(),
        ),
        (FileId(900), "empty.jsonl", b"".as_slice()),
    ];
    let mut shards = Vec::new();
    let mut direct = SegmentAdmission::new(CollectionId(0), &source);
    for (id, name, bytes) in files {
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
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        let mut admission = SegmentAdmission::new(CollectionId(0), &source);
        admission
            .admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        shards.push(
            admission
                .seal()
                .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
                .unwrap(),
        );
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        direct
            .admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
    }
    let direct = direct
        .seal()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let refs = shards.iter().collect::<Vec<_>>();
    let merged = IndexedCollectionSegment::consolidate(&refs, 100).unwrap();
    assert_eq!(merged.offsets, [0, 2, 4]);
    let first_active = crate::inventory::ActiveRecords::all(2).unwrap();
    let second_active = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(1)])
        .unwrap();
    let empty_active = crate::inventory::ActiveRecords::all(0).unwrap();
    let active = merged
        .translate_active(&[&first_active, &second_active, &empty_active])
        .unwrap();
    assert_eq!(active.len(), 3);
    assert!(!active.contains(RecordSlot(3)));
    assert!(active.contains(RecordSlot(2)));
    let merged = merged.index;
    assert_eq!(merged.records.len(), 4);
    assert_eq!(
        merged
            .files
            .iter()
            .map(|file| file.entry.id)
            .collect::<Vec<_>>(),
        [FileId(100), FileId(500), FileId(900)]
    );
    assert_eq!(
        merged.file_records(FileId(500)).unwrap(),
        [RecordSlot(2), RecordSlot(3)]
    );
    assert_eq!(merged.logical_records, direct.logical_records);
    assert_eq!(merged.empty_array_records, direct.empty_array_records);
    assert_eq!(
        merged.postings.fields[&vec!["n".to_owned()]].values,
        direct.postings.fields[&vec!["n".to_owned()]].values
    );
    assert_eq!(merged.raw.normal, direct.raw.normal);
    assert_eq!(merged.fallback_text.normal, direct.fallback_text.normal);
    assert_eq!(
        merged.sorting.columns[&FieldPath::parse("n").unwrap()].ascending,
        direct.sorting.columns[&FieldPath::parse("n").unwrap()].ascending
    );
    assert_eq!(
        merged.sorting.columns[&FieldPath::parse("n").unwrap()].descending,
        direct.sorting.columns[&FieldPath::parse("n").unwrap()].descending
    );
    assert_eq!(
        merged.sorting.composites[0].order,
        direct.sorting.composites[0].order
    );
    assert_eq!(merged.identity_order, direct.identity_order);
    assert_eq!(merged.relations.len(), 1);
    assert_eq!(
        merged.relations[0].references().collect::<Vec<_>>(),
        direct.relations[0].references().collect::<Vec<_>>()
    );
    assert_eq!(
        merged.relations[0].unavailable(),
        direct.relations[0].unavailable()
    );
    let inferred = merged.inference.as_ref().unwrap();
    inferred
        .validate(
            inference,
            &merged.logical_records,
            merged.records.len(),
            &definition.schema,
        )
        .unwrap();
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]].values,
        direct.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].values
    );
    assert!(IndexedCollectionSegment::consolidate(&refs, 3).is_err());
    assert!(
        IndexedCollectionSegment::consolidate(&[&shards[0], &shards[0]], 100)
            .err()
            .unwrap()
            .contains("overlapping physical file")
    );
}

#[test]
fn seventy_small_shards_consolidate_after_sources_are_removed() {
    let root = std::env::temp_dir().join(format!("objekte-segment-seventy-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
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
    let mut shards = Vec::new();
    for number in 0..70u32 {
        let name = format!("{number}.jsonl");
        let path = root.join(&name);
        std::fs::write(&path, format!("{{\"n\":{number}}}\n")).unwrap();
        let input = SourceFileInput {
            id: FileId(10_000 + number),
            relative_path: name.into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
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
            .admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        shards.push(
            admission
                .seal()
                .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
                .unwrap(),
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    let refs = shards.iter().collect::<Vec<_>>();
    let started = Instant::now();
    let consolidated = IndexedCollectionSegment::consolidate(&refs, 70).unwrap();
    eprintln!(
        "70 one-record shard consolidation: {:.1} µs",
        started.elapsed().as_secs_f64() * 1_000_000.0
    );
    assert_eq!(consolidated.offsets.len(), 70);
    assert_eq!(consolidated.offsets[69], 69);
    assert_eq!(consolidated.index.files.len(), 70);
    assert_eq!(consolidated.index.records.len(), 70);
    for number in 0..70u32 {
        assert_eq!(
            consolidated
                .index
                .file_records(FileId(10_000 + number))
                .unwrap(),
            [RecordSlot(number)]
        );
    }
}

#[test]
fn compaction_matches_active_file_oracle_across_indexes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-compact-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    for (name, kind, indexed) in [
        ("n", LogicalType::Number, true),
        ("ref", LogicalType::Id, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "target".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
        .sort_fields
        .push(FieldPath::parse("ref").unwrap());
    definition
        .schema
        .register_composite_sort(vec![
            crate::workload::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::workload::SortField {
                field: "ref".into(),
                descending: true,
                expression: None,
            },
        ])
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut all = SegmentAdmission::new(CollectionId(0), &source);
    let mut only_live = SegmentAdmission::new(CollectionId(0), &source);
    for (id, name, bytes) in [
        (
            FileId(100),
            "retired.jsonl",
            b"{\"n\":1,\"extra\":11,\"ref\":\"x\"}\n{bad}\n".as_slice(),
        ),
        (
            FileId(200),
            "live.jsonl",
            b"{\"n\":2,\"extra\":22,\"ref\":\"y\"}\n".as_slice(),
        ),
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
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        all.admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        if id == FileId(200) {
            let prepared = prepare_file_build(
                &root,
                input.clone(),
                &[binding],
                BigramAlphabet::Ascii,
                inference,
                1,
            )
            .unwrap();
            only_live
                .admit(&input, prepared.collections.into_iter().next().unwrap())
                .unwrap();
        }
    }
    let all = all
        .seal()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    let oracle = only_live
        .seal()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let active = crate::inventory::ActiveRecords::all(3)
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let compacted = all
        .compact_active(&active, 3, |file| file.id == FileId(200))
        .unwrap();
    assert_eq!(compacted.old_to_new, [None, None, Some(RecordSlot(0))]);
    assert_eq!(compacted.active.len(), 1);
    let compacted = compacted.index;
    assert_eq!(compacted.records.len(), 1);
    assert_eq!(compacted.files.len(), 1);
    assert_eq!(compacted.files[0].entry.id, FileId(200));
    assert_eq!(compacted.id_lookup, oracle.id_lookup);
    assert_eq!(compacted.logical_records, oracle.logical_records);
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]].values,
        oracle.postings.fields[&vec!["n".to_owned()]].values
    );
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .postings,
        oracle.postings.fields[&vec!["n".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .postings,
    );
    assert_eq!(compacted.raw.normal, oracle.raw.normal);
    assert_eq!(compacted.fallback_text.normal, oracle.fallback_text.normal);
    assert_eq!(
        compacted.sorting.columns[&FieldPath::parse("n").unwrap()].ascending,
        oracle.sorting.columns[&FieldPath::parse("n").unwrap()].ascending
    );
    assert_eq!(
        compacted.sorting.composites[0].order,
        oracle.sorting.composites[0].order
    );
    assert_eq!(
        compacted.relations[0].references().collect::<Vec<_>>(),
        oracle.relations[0].references().collect::<Vec<_>>()
    );
    let inferred = compacted.inference.as_ref().unwrap();
    inferred
        .validate(
            inference,
            &compacted.logical_records,
            compacted.records.len(),
            &definition.schema,
        )
        .unwrap();
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]].values,
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].values
    );
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .postings,
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .postings,
    );
    assert!(
        all.compact_active(&active, 2, |file| file.id == FileId(200))
            .is_err()
    );
}

#[test]
fn historical_file_versions_compact_to_current_zero_record_or_delete() {
    let root =
        std::env::temp_dir().join(format!("objekte-segment-versions-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("same.jsonl");
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
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
    let mut versions = Vec::new();
    for bytes in [
        b"{\"n\":1}\n{\"n\":2}\n".as_slice(),
        b"{\"n\":3}\n".as_slice(),
        b"".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id: FileId(77),
            relative_path: "same.jsonl".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
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
            .admit(&input, prepared.collections.into_iter().next().unwrap())
            .unwrap();
        versions.push(
            admission
                .seal()
                .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
                .unwrap(),
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    let old = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0), RecordSlot(1)])
        .unwrap();
    let replaced = crate::inventory::ActiveRecords::all(1)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let zero = crate::inventory::ActiveRecords::all(0).unwrap();
    let compacted = [
        versions[0].compact_active(&old, 2, |_| false).unwrap(),
        versions[1].compact_active(&replaced, 1, |_| false).unwrap(),
        versions[2].compact_active(&zero, 0, |_| true).unwrap(),
    ];
    assert!(compacted[0].index.files.is_empty());
    assert!(compacted[1].index.files.is_empty());
    assert_eq!(compacted[2].index.files.len(), 1);
    assert!(compacted.iter().all(|part| part.index.records.is_empty()));
    let refs = compacted.iter().map(|part| &part.index).collect::<Vec<_>>();
    let combined = IndexedCollectionSegment::consolidate(&refs, 0).unwrap();
    assert_eq!(combined.index.files.len(), 1);
    assert_eq!(combined.index.files[0].entry.id, FileId(77));
    assert_eq!(
        combined.index.files[0].entry.revision,
        versions[2].files[0].entry.revision
    );
    assert!(combined.index.records.is_empty());
    let deleted = versions[2].compact_active(&zero, 0, |_| false).unwrap();
    assert!(deleted.index.files.is_empty());
    assert!(deleted.index.records.is_empty());
    assert!(versions[0].compact_active(&old, 1, |_| false).is_err());
}

#[test]
fn compaction_remaps_retained_fallback_diagnostics() {
    let root =
        std::env::temp_dir().join(format!("objekte-segment-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("broken.jsonl");
    std::fs::write(&path, b"{bad}\n{worse}\n").unwrap();
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let input = SourceFileInput {
        id: FileId(66),
        relative_path: "broken.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
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
        .admit(&input, prepared.collections.into_iter().next().unwrap())
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = admission
        .seal()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let active = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let compacted = indexed.compact_active(&active, 2, |_| true).unwrap();
    assert_eq!(compacted.old_to_new, [None, Some(RecordSlot(0))]);
    assert_eq!(compacted.index.records.len(), 1);
    assert_eq!(compacted.index.records[0].diagnostic_ref, DiagnosticRef(2));
    assert_eq!(compacted.index.files[0].entry.diagnostics.len(), 2);
    assert_eq!(compacted.index.files[0].entry.diagnostics[0].record, None);
    assert_eq!(
        compacted.index.files[0].entry.diagnostics[1].record,
        Some(RecordSlot(0))
    );
    assert_eq!(compacted.index.files[0].entry.status, FileStatus::Broken);
    assert_eq!(
        compacted
            .index
            .fallback_text
            .candidates("worse")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
}
