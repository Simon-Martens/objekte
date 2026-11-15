use super::*;
use crate::inference::{InferenceMode, InferencePolicy};
use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::source::{SourceFormat, SourceRegistry};
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
    assert_eq!(indexed.record_count(), 2);
    assert_eq!(indexed.file_at(0).unwrap().records, [RecordSlot(0)]);
    assert_eq!(indexed.file_at(1).unwrap().records, [RecordSlot(1)]);
    assert_eq!(indexed.file_at(1).unwrap().entry.diagnostics.len(), 1);
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
    assert_eq!(indexed.record_count(), 3);
    assert_eq!(indexed.logical_records().iter().collect::<Vec<_>>(), [0, 1]);
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
        indexed.postings.fields[&vec!["n".to_owned()]].value_entries()
            [&ScalarKey::Number(Number::integer(2))]
            .iter()
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(
        indexed.sorting.columns[&FieldPath::parse("n").unwrap()]
            .ascending()
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
        assert_eq!(indexed.record_count(), 1);
        assert_eq!(indexed.file_at(0).unwrap().entry.id, FileId(700));
        assert_eq!(indexed.logical_records().iter().collect::<Vec<_>>(), [0]);
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
            .finish()
            .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
            .unwrap();
        assert_eq!(indexed.record_count(), 1);
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
                .finish()
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
        .finish()
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
        merged.postings.fields[&vec!["n".to_owned()]].value_entries(),
        direct.postings.fields[&vec!["n".to_owned()]].value_entries()
    );
    assert_eq!(merged.raw.normal_entries(), direct.raw.normal_entries());
    assert_eq!(merged.fallback_text.normal_entries(), direct.fallback_text.normal_entries());
    assert_eq!(
        merged.sorting.columns[&FieldPath::parse("n").unwrap()].ascending(),
        direct.sorting.columns[&FieldPath::parse("n").unwrap()].ascending()
    );
    assert_eq!(
        merged.sorting.columns[&FieldPath::parse("n").unwrap()].descending(),
        direct.sorting.columns[&FieldPath::parse("n").unwrap()].descending()
    );
    assert_eq!(
        merged.sorting.composites[0].order(),
        direct.sorting.composites[0].order()
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
        inferred.indexes[&vec!["extra".to_owned()]].value_entries(),
        direct.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].value_entries()
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
                .finish()
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
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    let oracle = only_live
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    let masks = [
        crate::inventory::ActiveRecords::from_bitmap(3, RoaringBitmap::from_iter([0, 2])).unwrap(),
        crate::inventory::ActiveRecords::from_bitmap(3, RoaringBitmap::from_iter([1])).unwrap(),
    ];
    let routing = PartitionRouting::new(&all, &masks).unwrap();
    let scattered = all.project_sorting_for_partitions(&routing).unwrap();
    let domains = all.project_domains_for_partitions(&routing).unwrap();
    let scattered_inference = all.project_inference_for_partitions(&routing, &domains).unwrap();
    for (part, sorting) in scattered.iter().enumerate() {
        let slot_map = SlotMap::Partition { routing: &routing, part };
        assert_eq!(domains[part].logical, project_bitmap(&all.logical_records, &slot_map).unwrap());
        assert_eq!(domains[part].empty_array, project_bitmap(&all.empty_array_records, &slot_map).unwrap());
        let records = routing.selected[part].iter().enumerate().map(|(new, &old)| {
            let mut record = all.record(RecordSlot(old)).unwrap().clone();
            record.slot = RecordSlot(new as u32);
            record
        }).collect::<Vec<_>>();
        let reference_inference = project_inference(
            &all.inference, &slot_map, &domains[part].logical,
            records.len(), slot_map.bounds().unwrap(),
        ).unwrap();
        let actual_inference = scattered_inference[part].as_ref().unwrap();
        let expected_inference = reference_inference.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&actual_inference.report).unwrap(),
            serde_json::to_value(&expected_inference.report).unwrap(),
        );
        assert_eq!(actual_inference.empty_selections, expected_inference.empty_selections);
        assert_eq!(actual_inference.short_string_coverage, expected_inference.short_string_coverage);
        assert_eq!(actual_inference.indexes.len(), expected_inference.indexes.len());
        for (path, actual) in &actual_inference.indexes {
            let expected = &expected_inference.indexes[path];
            assert_eq!(actual.presence, expected.presence);
            assert_eq!(actual.empty_selections, expected.empty_selections);
            assert_eq!(actual.nulls, expected.nulls);
            assert_eq!(actual.value_entries(), expected.value_entries());
            assert_eq!(actual.length_entries(), expected.length_entries());
            assert_eq!(actual.structural.as_ref().map(|facts| facts.entries()),
                expected.structural.as_ref().map(|facts| facts.entries()));
        }
        let reference = project_sorting(
            &all.sorting,
            &slot_map,
            &records,
        ).unwrap();
        for (path, column) in &sorting.columns {
            let expected = &reference.columns[path];
            assert_eq!(column.ascending(), expected.ascending());
            assert_eq!(column.descending(), expected.descending());
        }
        for (actual, expected) in sorting.composites.iter().zip(&reference.composites) {
            assert_eq!(actual.order(), expected.order());
        }
    }
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
    assert_eq!(compacted.file_count(), 1);
    assert_eq!(compacted.file_at(0).unwrap().entry.id, FileId(200));
    assert_eq!(compacted.id_lookup, oracle.id_lookup.clone());
    assert_eq!(compacted.logical_records, oracle.logical_records);
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]].value_entries(),
        oracle.postings.fields[&vec!["n".to_owned()]].value_entries()
    );
    assert_eq!(
        compacted.postings.fields[&vec!["n".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .entries(),
        oracle.postings.fields[&vec!["n".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .entries(),
    );
    assert_eq!(compacted.raw.normal_entries(), oracle.raw.normal_entries());
    assert_eq!(compacted.fallback_text.normal_entries(), oracle.fallback_text.normal_entries());
    assert_eq!(
        compacted.sorting.columns[&FieldPath::parse("n").unwrap()].ascending(),
        oracle.sorting.columns[&FieldPath::parse("n").unwrap()].ascending()
    );
    assert_eq!(
        compacted.sorting.composites[0].order(),
        oracle.sorting.composites[0].order()
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
        inferred.indexes[&vec!["extra".to_owned()]].value_entries(),
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]].value_entries()
    );
    assert_eq!(
        inferred.indexes[&vec!["extra".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .entries(),
        oracle.inference.as_ref().unwrap().indexes[&vec!["extra".to_owned()]]
            .structural
            .as_ref()
            .unwrap()
            .entries(),
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
                .finish()
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
    assert_eq!(combined.index.file_count(), 1);
    assert_eq!(combined.index.file_at(0).unwrap().entry.id, FileId(77));
    assert_eq!(
        combined.index.file_at(0).unwrap().entry.revision,
        versions[2].file_at(0).unwrap().entry.revision
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
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let active = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let compacted = indexed.compact_active(&active, 2, |_| true).unwrap();
    assert_eq!(compacted.old_to_new, [None, Some(RecordSlot(0))]);
    assert_eq!(compacted.index.record_count(), 1);
    assert_eq!(compacted.index.record(RecordSlot(0)).unwrap().diagnostic_ref, DiagnosticRef(2));
    assert_eq!(compacted.index.file_at(0).unwrap().entry.diagnostics.len(), 2);
    assert_eq!(compacted.index.file_at(0).unwrap().entry.diagnostics[0].record, None);
    assert_eq!(
        compacted.index.file_at(0).unwrap().entry.diagnostics[1].record,
        Some(RecordSlot(0))
    );
    assert_eq!(compacted.index.file_at(0).unwrap().entry.status, FileStatus::Broken);
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

fn small_number_shard(tag: &str, file_id: u32) -> IndexedCollectionSegment {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-{tag}-{}-{}",
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
    let name = format!("{tag}.jsonl");
    std::fs::write(root.join(&name), "{\"n\":1}\n{\"n\":2}\n").unwrap();
    let input = SourceFileInput {
        id: FileId(file_id),
        relative_path: name.clone().into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(root.join(&name)).unwrap())
            .unwrap(),
        created_ns: None,
    };
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let prepared = prepare_file_build(
        &root,
        input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
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
        .finish()
        .into_indexed(
            &definition,
            BigramAlphabet::Ascii,
            InferenceConfig::default(),
            1,
            &pool,
        )
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    indexed
}

fn freeze_all_postings(shard: &mut IndexedCollectionSegment) {
    for index in shard.postings.fields.values_mut() {
        index.freeze_shared();
    }
    for custom in shard.custom_postings.values_mut() {
        custom.index.freeze_shared();
    }
    if let Some(inference) = &mut shard.inference {
        for index in inference.indexes.values_mut() {
            index.freeze_shared();
        }
    }
}

#[test]
fn sealed_shard_backing_follows_policy() {
    let shard = small_number_shard("policy", 0);
    let expected = cfg!(feature = "force-shared-backing");
    assert!(!shard.postings.fields.is_empty());
    for index in shard.postings.fields.values() {
        let facts = index.structural.as_ref().unwrap();
        assert_eq!(facts.is_shared(), expected);
        assert_eq!(index.postings_are_shared(), expected);
    }
    assert_eq!(shard.raw.grams_are_shared(), expected);
    assert_eq!(shard.fallback_text.grams_are_shared(), expected);
    assert_eq!(shard.files.is_shared(), expected);
    assert_eq!(shard.file_index.is_shared(), expected);
    assert_eq!(shard.records.is_shared(), expected);
    assert_eq!(shard.id_lookup.is_shared(), expected);
    assert_eq!(shard.identity_order.is_shared(), expected);
    for column in shard.sorting.columns.values() {
        assert_eq!(column.is_shared(), expected);
    }
    for composite in &shard.sorting.composites {
        assert_eq!(composite.is_shared(), expected);
    }
    for relation in shard.relations() {
        assert_eq!(relation.references_are_shared(), expected);
    }
}

#[test]
fn consolidation_of_frozen_and_native_shards_matches_native() {
    let a = small_number_shard("frozen-a", 0);
    let b = small_number_shard("frozen-b", 10);
    let expected = IndexedCollectionSegment::consolidate(&[&a, &b], usize::MAX).unwrap();
    let mut frozen_a = small_number_shard("frozen-a", 0);
    freeze_all_postings(&mut frozen_a);
    let actual = IndexedCollectionSegment::consolidate(&[&frozen_a, &b], usize::MAX).unwrap();
    assert_eq!(
        actual.index.accounting.retained_bytes_estimated,
        expected.index.accounting.retained_bytes_estimated
    );
    for (path, field) in &expected.index.postings.fields {
        let other = &actual.index.postings.fields[path];
        assert_eq!(field.value_entries(), other.value_entries());
        assert_eq!(field.length_entries(), other.length_entries());
        assert_eq!(
            field.structural.as_ref().unwrap().entries(),
            other.structural.as_ref().unwrap().entries()
        );
    }
}

#[test]
#[ignore = "manual real-fixture shared-backing measurement; run in release with --nocapture"]
fn measure_shared_backing_on_real_fixture() {
    let root = std::path::PathBuf::from(
        std::env::var_os("OBJEKTE_SHARED_BACKING_FIXTURE")
            .expect("set OBJEKTE_SHARED_BACKING_FIXTURE to a Musenalm root"),
    );
    let configuration =
        crate::repository_configuration::load_repository_configuration(&root).unwrap();
    let repository = crate::repository::Repository::open_with_options(
        &root,
        configuration.definitions,
        crate::repository::RepositoryOptions::default(),
    )
    .unwrap();
    let collection = repository
        .collections()
        .iter()
        .find(|collection| collection.definition.name == "contents")
        .unwrap();
    let record_count = collection.generation.records.len();
    eprintln!("records={record_count}");

    let median = |mut samples: Vec<f64>| {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        samples[samples.len() / 2]
    };

    let native = crate::lifecycle_compaction::base_as_segment(collection, repository.files())
        .unwrap();

    let mut freeze_samples = Vec::new();
    for _ in 0..5 {
        let mut shard =
            crate::lifecycle_compaction::base_as_segment(collection, repository.files()).unwrap();
        let started = Instant::now();
        shard.freeze_all();
        freeze_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let freeze_ms = median(freeze_samples);

    let mut shared =
        crate::lifecycle_compaction::base_as_segment(collection, repository.files()).unwrap();
    shared.freeze_all();
    shared.recompute_accounting();

    let posting_keys = shared
        .postings
        .fields
        .values()
        .map(|index| index.distinct_values() + index.distinct_lengths())
        .sum::<usize>();
    let posting_chunks = shared
        .postings
        .fields
        .values()
        .map(crate::posting::PostingIndex::stored_chunk_count)
        .sum::<usize>();
    let gram_keys = shared.raw.gram_key_count()
        + shared.fallback_text.gram_key_count()
        + shared
            .custom_text
            .values()
            .map(crate::bigram::BigramIndex::gram_key_count)
            .sum::<usize>();
    let gram_chunks = shared.raw.stored_chunk_count()
        + shared.fallback_text.stored_chunk_count()
        + shared
            .custom_text
            .values()
            .map(crate::bigram::BigramIndex::stored_chunk_count)
            .sum::<usize>();

    let (field_path, shared_field) = shared
        .postings
        .fields
        .iter()
        .max_by_key(|(_, index)| index.distinct_values())
        .unwrap();
    let native_field = &native.postings.fields[field_path];

    let keys = shared_field.value_entries().into_keys().collect::<Vec<_>>();
    let lookup = |field: &crate::posting::PostingIndex| {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for i in 0..10_000usize {
                let _ = field.value_posting(&keys[i % keys.len()]);
            }
            samples.push(started.elapsed().as_secs_f64() * 1e9 / 10_000.0);
        }
        median(samples)
    };
    let lookup_native_ns = lookup(native_field);
    let lookup_shared_ns = lookup(shared_field);

    let scan = |field: &crate::posting::PostingIndex| {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let mut memberships = 0u64;
            field
                .for_each_value(|_, slots| {
                    memberships += slots.len();
                    Ok(())
                })
                .unwrap();
            std::hint::black_box(memberships);
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        median(samples)
    };
    let scan_native_ms = scan(native_field);
    let scan_shared_ms = scan(shared_field);

    let literals = shared
        .records
        .iter()
        .take(100)
        .map(|record| {
            let id = &record.id.0;
            let start = id.len().saturating_sub(8);
            id[start..].to_string()
        })
        .collect::<Vec<_>>();
    let fts = |index: &crate::bigram::BigramIndex| {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for literal in &literals {
                let _ = index.candidates(literal);
            }
            samples.push(started.elapsed().as_secs_f64() * 1e6);
        }
        median(samples)
    };
    let fts_native_us = fts(&native.raw);
    let fts_shared_us = fts(&shared.raw);

    let mut clone_samples = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let cloned = (
            shared.postings.clone(),
            shared.custom_postings.clone(),
            shared.raw.clone(),
            shared.fallback_text.clone(),
            shared.custom_text.clone(),
            shared.inference.clone(),
        );
        std::hint::black_box(cloned);
        clone_samples.push(started.elapsed().as_secs_f64() * 1e9);
    }
    let clone_shared_ns = median(clone_samples);

    let mut cow_value_samples = Vec::new();
    for _ in 0..5 {
        let mut working = shared_field.clone();
        let started = Instant::now();
        for i in 0..1_000u32 {
            working.insert_value_slot(
                keys[i as usize % keys.len()].clone(),
                shared.record_count() as u32 + i,
            );
        }
        std::hint::black_box(&working);
        cow_value_samples.push(started.elapsed().as_secs_f64() * 1e6 / 1_000.0);
    }
    let cow_value_us = median(cow_value_samples);

    let gram_keys_for_reuse = shared
        .raw
        .normal_entries()
        .into_keys()
        .collect::<Vec<_>>();
    let mut cow_gram_samples = Vec::new();
    for _ in 0..5 {
        let mut working = shared.raw.clone();
        let started = Instant::now();
        for i in 0..1_000u32 {
            working.insert_gram_slot(
                false,
                gram_keys_for_reuse[i as usize % gram_keys_for_reuse.len()],
                shared.record_count() as u32 + i,
            );
        }
        std::hint::black_box(&working);
        cow_gram_samples.push(started.elapsed().as_secs_f64() * 1e6 / 1_000.0);
    }
    let cow_gram_us = median(cow_gram_samples);

    eprintln!("freeze_ms={freeze_ms:.3}");
    eprintln!("keys={posting_keys} chunks={posting_chunks} gram_keys={gram_keys} gram_chunks={gram_chunks}");
    eprintln!(
        "estimate_native_bytes={} estimate_shared_bytes={}",
        native.accounting.index_family_bytes, shared.accounting.index_family_bytes
    );
    eprintln!("lookup_native_ns={lookup_native_ns:.1} lookup_shared_ns={lookup_shared_ns:.1}");
    eprintln!("scan_native_ms={scan_native_ms:.3} scan_shared_ms={scan_shared_ms:.3}");
    eprintln!("fts_native_us={fts_native_us:.1} fts_shared_us={fts_shared_us:.1}");
    eprintln!("clone_shared_ns={clone_shared_ns:.1}");
    eprintln!("cow_value_us={cow_value_us:.3} cow_gram_us={cow_gram_us:.3}");
    eprintln!(
        "field={:?} keys_in_field={} field_records={}",
        field_path,
        keys.len(),
        shared_field.distinct_values()
    );
}

fn append_fixture_shard(tag: &str, file_id: u32, record_count: usize) -> IndexedCollectionSegment {
    append_fixture_shard_records(tag, file_id, &(0..record_count).collect::<Vec<_>>())
}

fn append_fixture_shard_records(
    tag: &str,
    file_id: u32,
    numbers: &[usize],
) -> IndexedCollectionSegment {
    let root = std::env::temp_dir().join(format!(
        "objekte-append-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    for (path, kind, multi) in [
        ("name", LogicalType::String, false),
        ("n", LogicalType::Number, false),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
        ("ref", LogicalType::Id, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi,
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
    definition
        .schema
        .register_custom(
            vec!["name".into()],
            crate::reference::CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: crate::reference::Transform::CaseFold,
            },
        )
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("name").unwrap());
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
        .schema
        .register_composite_sort(vec![
            crate::workload::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::workload::SortField {
                field: "name".into(),
                descending: true,
                expression: None,
            },
        ])
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let name = format!("{tag}.jsonl");
    let body = numbers
        .iter()
        .map(|number| {
            format!(
                "{{\"name\":\"name-{number}\",\"n\":{number},\"tags\":[\"tag-a\",\"tag-b\"],\"ref\":\"target-{}\",\"extra\":{number}}}\n",
                number % 5
            )
        })
        .collect::<String>();
    std::fs::write(root.join(&name), body).unwrap();
    let input = SourceFileInput {
        id: FileId(file_id),
        relative_path: name.clone().into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(root.join(&name)).unwrap())
            .unwrap(),
        created_ns: None,
    };
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
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
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    indexed
}

fn assert_shards_equal(actual: &IndexedCollectionSegment, expected: &IndexedCollectionSegment) {
    assert_eq!(actual.record_count(), expected.record_count());
    for slot in 0..actual.record_count() {
        assert_eq!(
            format!("{:?}", actual.record(RecordSlot(slot as u32))),
            format!("{:?}", expected.record(RecordSlot(slot as u32)))
        );
    }
    assert_eq!(actual.file_count(), expected.file_count());
    for position in 0..actual.file_count() {
        let left = actual.file_at(position).unwrap();
        let right = expected.file_at(position).unwrap();
        assert_eq!(format!("{:?}", left.entry), format!("{:?}", right.entry));
        assert_eq!(left.records, right.records);
    }
    let ids = |shard: &IndexedCollectionSegment| {
        shard
            .ids_iter()
            .map(|(id, slot)| (id.clone(), slot))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(ids(actual), ids(expected));
    assert_eq!(actual.identity_order().to_vec(), expected.identity_order().to_vec());
    assert_eq!(actual.logical_records(), expected.logical_records());
    assert_eq!(actual.empty_array_records(), expected.empty_array_records());
    for (path, field) in &expected.postings.fields {
        let other = &actual.postings.fields[path];
        assert_eq!(other.value_entries(), field.value_entries());
        assert_eq!(other.length_entries(), field.length_entries());
        assert_eq!(
            other.structural.as_ref().unwrap().entries(),
            field.structural.as_ref().unwrap().entries()
        );
        assert_eq!(other.presence, field.presence);
        assert_eq!(other.nulls, field.nulls);
        assert_eq!(other.empty_selections, field.empty_selections);
        assert_eq!(other.covered_records, field.covered_records);
        assert_eq!(other.stats.present_count, field.stats.present_count);
        assert_eq!(other.stats.null_count, field.stats.null_count);
        assert_eq!(other.stats.distinct_count, field.stats.distinct_count);
        assert_eq!(other.stats.min_key, field.stats.min_key);
        assert_eq!(other.stats.max_key, field.stats.max_key);
    }
    assert_eq!(actual.custom_postings.len(), expected.custom_postings.len());
    for (key, custom) in &expected.custom_postings {
        let other = &actual.custom_postings[key];
        assert_eq!(other.index.value_entries(), custom.index.value_entries());
        assert_eq!(other.complete, custom.complete);
    }
    match (&actual.inference, &expected.inference) {
        (None, None) => {}
        (Some(left), Some(right)) => {
            assert_eq!(format!("{:?}", left.report), format!("{:?}", right.report));
            let paths = left
                .indexes
                .keys()
                .chain(right.indexes.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for path in paths {
                assert_eq!(left.indexes.get(path).map(|i| i.value_entries()), right.indexes.get(path).map(|i| i.value_entries()));
                assert_eq!(left.empty_selections.get(path), right.empty_selections.get(path));
                assert_eq!(left.short_string_coverage.get(path), right.short_string_coverage.get(path));
            }
        }
        _ => panic!("inference presence differs"),
    }
    let grams = |index: &BigramIndex| {
        (
            index.normal_entries(),
            index.skip_entries(),
            index.record_count,
        )
    };
    assert_eq!(grams(&actual.raw), grams(&expected.raw));
    assert_eq!(grams(&actual.fallback_text), grams(&expected.fallback_text));
    assert_eq!(actual.custom_text.len(), expected.custom_text.len());
    for (key, index) in &expected.custom_text {
        assert_eq!(grams(&actual.custom_text[key]), grams(index));
    }
    assert_eq!(
        actual.sorting.columns.len(),
        expected.sorting.columns.len()
    );
    for (path, column) in &expected.sorting.columns {
        let other = &actual.sorting.columns[path];
        assert_eq!(
            other.keys_iter().cloned().collect::<Vec<_>>(),
            column.keys_iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(other.ascending().to_vec(), column.ascending().to_vec());
        assert_eq!(other.descending().to_vec(), column.descending().to_vec());
    }
    assert_eq!(
        actual.sorting.composites.len(),
        expected.sorting.composites.len()
    );
    for (left, right) in actual.sorting.composites.iter().zip(&expected.sorting.composites) {
        assert_eq!(left.order().to_vec(), right.order().to_vec());
    }
    assert_eq!(actual.relations.len(), expected.relations.len());
    for (left, right) in actual.relations.iter().zip(&expected.relations) {
        assert_eq!(left.present(), right.present());
        assert_eq!(left.nulls(), right.nulls());
        assert_eq!(left.empty_arrays(), right.empty_arrays());
        assert_eq!(left.unavailable(), right.unavailable());
        assert_eq!(
            left.references().collect::<Vec<_>>(),
            right.references().collect::<Vec<_>>()
        );
    }
    assert_eq!(actual.record_parses, expected.record_parses);
    assert_eq!(actual.failed_parses, expected.failed_parses);
}

#[test]
fn append_matches_consolidate() {
    let a = append_fixture_shard("append-base", 1, 300);
    for count in [1usize, 2, 40] {
        let b = append_fixture_shard(&format!("append-delta-{count}"), 2, count);
        let expected = IndexedCollectionSegment::consolidate(&[&a, &b], usize::MAX).unwrap();
        let actual = a
            .to_shared()
            .append(&ActiveRecords::all(a.record_count()).unwrap(), &b)
            .unwrap();
        assert_eq!(actual.base, a.record_count() as u32);
        assert_eq!(actual.index.role, ShardRole::Main);
        assert_shards_equal(&actual.index, &expected.index);
    }
}

#[test]
fn repeated_append_matches_one_consolidate() {
    let a = append_fixture_shard("repeat-base", 1, 300);
    let deltas = (0..6)
        .map(|index| append_fixture_shard(&format!("repeat-delta-{index}"), 2 + index as u32, 1 + index))
        .collect::<Vec<_>>();
    let refs = std::iter::once(&a)
        .chain(deltas.iter())
        .collect::<Vec<_>>();
    let expected = IndexedCollectionSegment::consolidate(&refs, usize::MAX).unwrap();
    let mut main = a.to_shared();
    let mut active = ActiveRecords::all(a.record_count()).unwrap();
    for delta in &deltas {
        let appended = main.append(&active, delta).unwrap();
        active = active.concat(&ActiveRecords::all(delta.record_count()).unwrap()).unwrap();
        main = appended.index;
    }
    assert_shards_equal(&main, &expected.index);
}

#[test]
fn append_into_native_backing_matches_shared() {
    let a = append_fixture_shard("native-base", 1, 40);
    let b = append_fixture_shard("native-delta", 2, 3);
    let expected = IndexedCollectionSegment::consolidate(&[&a, &b], usize::MAX).unwrap();
    let mut native = a.clone();
    native.role = ShardRole::Main;
    let actual = native
        .append(&ActiveRecords::all(a.record_count()).unwrap(), &b)
        .unwrap();
    assert_shards_equal(&actual.index, &expected.index);
}

#[test]
fn append_does_not_change_the_original() {
    let a = append_fixture_shard("unchanged-base", 1, 20);
    let b = append_fixture_shard("unchanged-delta", 2, 2);
    let before = a.to_shared();
    let main = a.to_shared();
    main.append(&ActiveRecords::all(a.record_count()).unwrap(), &b)
        .unwrap();
    assert_shards_equal(&main, &before);
}

#[test]
fn append_replaces_a_retired_file() {
    let a = append_fixture_shard("replace-base", 7, 3);
    let active = ActiveRecords::all(a.record_count())
        .unwrap()
        .retired(&(0..a.record_count() as u32).map(RecordSlot).collect::<Vec<_>>())
        .unwrap();
    let b = append_fixture_shard("replace-delta", 7, 3);
    let main = a.to_shared();
    let appended = main.append(&active, &b).unwrap();
    let base = appended.base;
    let file = appended.index.file(FileId(7)).unwrap();
    assert_eq!(file.records, [RecordSlot(base), RecordSlot(base + 1), RecordSlot(base + 2)]);
    assert_eq!(appended.index.file_count(), a.file_count());
    for record in b.records_iter() {
        assert_eq!(
            appended.index.slot_for_id(record.id.0.as_ref()),
            Some(RecordSlot(base + record.slot.0))
        );
        assert!(appended.index.record(record.slot).is_some());
        assert!(appended
            .index
            .identity_order()
            .to_vec()
            .contains(&RecordSlot(base + record.slot.0)));
        for column in appended.index.sorting.columns.values() {
            assert!(column.ascending().to_vec().contains(&RecordSlot(base + record.slot.0)));
        }
    }
}

#[test]
fn append_rejects_an_active_duplicate() {
    let a = append_fixture_shard("duplicate-base", 7, 2);
    let b = append_fixture_shard("duplicate-delta", 7, 2);
    let main = a.to_shared();
    let active = ActiveRecords::all(a.record_count()).unwrap();
    assert_eq!(
        main.append(&active, &b).err().unwrap(),
        "append would duplicate an active file"
    );
    let id = a.record(RecordSlot(0)).unwrap().id.clone();
    let mut foreign = b.clone();
    foreign.files = crate::slot_store::SlotTable::new();
    foreign.file_index = crate::slot_store::SharedMap::new();
    foreign.records.get_mut(0).unwrap().id = id.clone();
    foreign.id_lookup.insert(id, RecordSlot(0));
    assert_eq!(
        main.append(&active, &foreign).err().unwrap(),
        "append would duplicate an active record ID"
    );
}

#[test]
fn append_rejects_a_foreign_collection() {
    let a = append_fixture_shard("foreign-base", 1, 2);
    let mut b = append_fixture_shard("foreign-delta", 2, 2);
    b.collection = CollectionId(1);
    let main = a.to_shared();
    assert_eq!(
        main.append(&ActiveRecords::all(a.record_count()).unwrap(), &b)
            .err().unwrap(),
        "segment collection or source binding mismatch"
    );
}

#[test]
fn append_handles_an_inference_type_conflict() {
    let a = append_fixture_shard("conflict-base", 1, 20);
    let mut b = append_fixture_shard("conflict-delta", 2, 20);
    b.inference
        .as_mut()
        .unwrap()
        .indexes
        .get_mut(&vec!["extra".to_owned()])
        .unwrap()
        .kind = LogicalType::String;
    let expected = IndexedCollectionSegment::consolidate(&[&a, &b], usize::MAX).unwrap();
    let actual = a
        .to_shared()
        .append(&ActiveRecords::all(a.record_count()).unwrap(), &b)
        .unwrap();
    assert_shards_equal(&actual.index, &expected.index);
}

fn assert_no_family_mentions(shard: &IndexedCollectionSegment, slots: &RoaringBitmap) {
    for index in shard.postings.fields.values() {
        for (_, posting) in index.value_entries() {
            assert!(posting.intersection_len(slots) == 0);
        }
        for (_, posting) in index.length_entries() {
            assert!(posting.intersection_len(slots) == 0);
        }
        if let Some(facts) = &index.structural {
            for (_, posting) in facts.entries() {
                assert!(posting.intersection_len(slots) == 0);
            }
        }
        assert!(index.presence.intersection_len(slots) == 0);
        assert!(index.nulls.intersection_len(slots) == 0);
        assert!(index.empty_selections.intersection_len(slots) == 0);
    }
    assert!(shard.logical_records().intersection_len(slots) == 0);
    assert!(shard.empty_array_records().intersection_len(slots) == 0);
    assert!(shard.identity_order().iter().all(|slot| !slots.contains(slot.0)));
    for column in shard.sorting.columns.values() {
        assert!(column.ascending().iter().all(|slot| !slots.contains(slot.0)));
        assert!(column.descending().iter().all(|slot| !slots.contains(slot.0)));
    }
    for composite in &shard.sorting.composites {
        assert!(composite.order().iter().all(|slot| !slots.contains(slot.0)));
    }
    for relation in shard.relations() {
        assert!(relation.present().intersection_len(slots) == 0);
        assert!(relation.nulls().intersection_len(slots) == 0);
        assert!(relation.empty_arrays().intersection_len(slots) == 0);
        assert!(relation.unavailable().intersection_len(slots) == 0);
        assert!(relation
            .reference_rows()
            .all(|(source, _, _)| !slots.contains(source.0)));
    }
    if let Some(inference) = &shard.inference {
        for index in inference.indexes.values() {
            for (_, posting) in index.value_entries() {
                assert!(posting.intersection_len(slots) == 0);
            }
            assert!(index.presence.intersection_len(slots) == 0);
        }
        for bitmap in inference.empty_selections.values() {
            assert!(bitmap.intersection_len(slots) == 0);
        }
    }
}

fn assert_shards_equivalent_by_id(
    actual: &IndexedCollectionSegment,
    actual_active: &ActiveRecords,
    expected: &IndexedCollectionSegment,
    expected_active: &ActiveRecords,
) {
    let live_ids = |shard: &IndexedCollectionSegment, active: &ActiveRecords| {
        (0..shard.record_count())
            .filter(|slot| active.contains(RecordSlot(*slot as u32)))
            .map(|slot| shard.record(RecordSlot(slot as u32)).unwrap().id.0.to_string())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(live_ids(actual, actual_active), live_ids(expected, expected_active));
    for path in actual.postings.fields.keys() {
        let left = &actual.postings.fields[path];
        let right = &expected.postings.fields[path];
        for (key, posting) in left.value_entries() {
            let ids: std::collections::BTreeSet<_> = posting
                .iter()
                .filter(|slot| actual_active.contains(RecordSlot(*slot)))
                .map(|slot| actual.record(RecordSlot(slot)).unwrap().id.0.to_string())
                .collect();
            let other: std::collections::BTreeSet<_> = right
                .value_entries()
                .get(&key)
                .map(|posting| {
                    posting
                        .iter()
                        .filter(|slot| expected_active.contains(RecordSlot(*slot)))
                        .map(|slot| expected.record(RecordSlot(slot)).unwrap().id.0.to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(ids, other);
        }
    }
    for column in actual.sorting.columns.values() {
        let expected_ids: Vec<_> = column
            .ascending()
            .iter()
            .filter(|slot| actual_active.contains(*slot))
            .map(|slot| actual.record(slot).unwrap().id.0.to_string())
            .collect();
        let right = &expected.sorting.columns[&column.path];
        let actual_expected: Vec<_> = right
            .ascending()
            .iter()
            .filter(|slot| expected_active.contains(*slot))
            .map(|slot| expected.record(slot).unwrap().id.0.to_string())
            .collect();
        assert_eq!(expected_ids, actual_expected);
    }
}

#[test]
fn vacuum_matches_compaction_by_id() {
    let base = append_fixture_shard("vacuum-base", 1, 30);
    let delta = append_fixture_shard("vacuum-delta", 2, 10);
    let main = base.to_shared();
    let active = ActiveRecords::all(base.record_count()).unwrap();
    let main = main.append(&active, &delta).unwrap().index;
    let retired = (0..12u32).map(RecordSlot).collect::<Vec<_>>();
    let active = ActiveRecords::all(main.record_count()).unwrap().retired(&retired).unwrap();
    let vacuumed = main.vacuum(&active, |_| true).unwrap().expect("vacuum");
    let expected = main.compact_active(&active, usize::MAX, |_| true).unwrap();
    assert_shards_equivalent_by_id(&vacuumed, &active, &expected.index, &expected.active);
}

#[test]
fn vacuum_frees_exactly_the_dead_slots() {
    let base = append_fixture_shard("vacuum-free", 1, 20);
    let main = base.to_shared();
    let retired = [0u32, 3, 7, 19].into_iter().map(RecordSlot).collect::<Vec<_>>();
    let active = ActiveRecords::all(main.record_count()).unwrap().retired(&retired).unwrap();
    let vacuumed = main
        .vacuum(&active, |_| true)
        .unwrap()
        .expect("vacuum");
    let dead = [0u32, 3, 7, 19].into_iter().collect::<RoaringBitmap>();
    assert_eq!(vacuumed.free_slots, dead);
    assert_no_family_mentions(&vacuumed, &dead);
    assert_eq!(vacuumed.live_slot_count(), 16);
}

#[test]
fn vacuum_without_dead_slots_returns_none_and_does_not_change_the_original() {
    let main = append_fixture_shard("vacuum-none", 1, 10).to_shared();
    let active = ActiveRecords::all(main.record_count()).unwrap();
    assert!(main.vacuum(&active, |_| true).unwrap().is_none());
    let retired = [RecordSlot(2), RecordSlot(4)];
    let active = active.retired(&retired).unwrap();
    let before = main.clone();
    let vacuumed = main.vacuum(&active, |_| true).unwrap().unwrap();
    let n = vec!["n".to_owned()];
    assert_eq!(
        main.postings.fields[&n].value_entries(),
        before.postings.fields[&n].value_entries()
    );
    let second = vacuumed.vacuum(&active, |_| true).unwrap();
    assert!(second.is_none());
}

#[test]
fn vacuum_drops_empty_keys_and_updates_stats() {
    let base = append_fixture_shard("vacuum-keys", 1, 6);
    let main = base.to_shared();
    let active = ActiveRecords::all(main.record_count())
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let vacuumed = main.vacuum(&active, |_| true).unwrap().unwrap();
    let field = &vacuumed.postings.fields[&vec!["n".to_owned()]];
    assert!(!field.value_entries().is_empty());
    assert_eq!(field.distinct_values(), field.value_entries().len());
    assert_eq!(field.stats.distinct_count, field.distinct_values());
}

#[test]
fn vacuum_marks_dead_file_rows_free() {
    let base = append_fixture_shard("vacuum-files", 1, 4);
    let delta = append_fixture_shard("vacuum-files-delta", 2, 2);
    let main = base.to_shared();
    let active = ActiveRecords::all(base.record_count()).unwrap();
    let main = main.append(&active, &delta).unwrap().index;
    let retired = (0..4u32).map(RecordSlot).collect::<Vec<_>>();
    let active = ActiveRecords::all(main.record_count()).unwrap().retired(&retired).unwrap();
    let vacuumed = main
        .vacuum(&active, |file| file.id != crate::model::FileId(1))
        .unwrap()
        .unwrap();
    assert_eq!(vacuumed.file(crate::model::FileId(1)).is_none(), true);
    assert_eq!(vacuumed.free_file_rows.len(), 1);
    assert!(vacuumed.file(crate::model::FileId(2)).is_some());
}

fn vacuumed_main(
    tag: &str,
    base_n: usize,
    delta_n: usize,
    retired: &[u32],
) -> (IndexedCollectionSegment, ActiveRecords) {
    let base = append_fixture_shard(&format!("{tag}-base"), 1, base_n);
    let delta = append_fixture_shard(&format!("{tag}-delta"), 2, delta_n);
    let main = base.to_shared();
    let active = ActiveRecords::all(base.record_count()).unwrap();
    let main = main.append(&active, &delta).unwrap().index;
    let retired = retired.iter().copied().map(RecordSlot).collect::<Vec<_>>();
    let active = ActiveRecords::all(main.record_count()).unwrap().retired(&retired).unwrap();
    let vacuumed = main.vacuum(&active, |_| true).unwrap().unwrap();
    (vacuumed, active)
}

#[test]
fn append_reuses_free_slots_first() {
    let (main, active) = vacuumed_main("reuse", 20, 10, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    let delta = append_fixture_shard("reuse-delta", 3, 4);
    let count = main.record_count();
    let freed = main.free_slots.len();
    let appended = main.append(&active, &delta).unwrap();
    assert_eq!(appended.index.record_count(), count);
    assert_eq!(appended.index.free_slots.len(), freed - 4);
    assert_eq!(appended.index.reuse_log.len(), 4);
    assert_eq!(appended.base, count as u32);
}

#[test]
fn append_overflows_into_new_slots() {
    let (main, active) = vacuumed_main("overflow", 20, 5, &[0, 2, 4]);
    let delta = append_fixture_shard("overflow-delta", 3, 5);
    let count = main.record_count();
    let appended = main.append(&active, &delta).unwrap();
    assert_eq!(appended.index.record_count(), count + 2);
    assert_eq!(appended.index.free_slots.len(), 0);
    assert_eq!(appended.index.reuse_log.len(), 3);
}

#[test]
fn append_rejects_reuse_of_an_active_slot() {
    let (main, active) = vacuumed_main("reuse-active", 10, 2, &[9]);
    let mut corrupted = main.clone();
    corrupted.free_slots.insert(0);
    let delta = append_fixture_shard("reuse-active-delta", 3, 2);
    assert_eq!(
        corrupted.append(&active, &delta).err().unwrap(),
        "append would reuse an active slot"
    );
}

#[test]
fn with_mapped_matches_bitmap_model() {
    let mapping = SlotMapping {
        reused: vec![RecordSlot(0), RecordSlot(3)],
        base: 5,
    };
    let left = ActiveRecords::all(2).unwrap();
    let right = ActiveRecords::all(3)
        .unwrap()
        .retired(&[RecordSlot(1)])
        .unwrap();
    let combined = left.with_mapped(&right, &mapping).unwrap();
    let mut expected = left.bitmap();
    for slot in right.bitmap().iter() {
        expected.insert(mapping.target(slot).0);
    }
    assert_eq!(combined.bitmap(), expected);
    assert_eq!(combined.slot_count(), mapping.slot_count(3));
}

#[test]
fn reused_slots_carry_only_the_new_record() {
    let (main, active) = vacuumed_main("reused", 20, 5, &[2, 4, 6, 8]);
    let delta = append_fixture_shard("reused-delta", 3, 3);
    let appended = main.append(&active, &delta).unwrap();
    let compacted = main.compact_active(&active, usize::MAX, |_| true).unwrap();
    let expected = IndexedCollectionSegment::consolidate(&[&compacted.index, &delta], usize::MAX)
        .unwrap();
    let active_after = active
        .with_mapped(&ActiveRecords::all(delta.record_count()).unwrap(), &appended.mapping)
        .unwrap();
    let delta_active = ActiveRecords::all(delta.record_count()).unwrap();
    let expected_active = expected
        .translate_active(&[&compacted.active, &delta_active])
        .unwrap();
    assert_shards_equivalent_by_id(
        &appended.index,
        &active_after,
        &expected.index,
        &expected_active,
    );
}
