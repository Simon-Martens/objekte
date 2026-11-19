use super::*;
use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
use crate::construction::segment_admission::SegmentAdmission;
use crate::index::bigram::BigramAlphabet;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::inventory::SourceRevision;
use crate::model::{CollectionId, FieldPath, LogicalType};
use crate::repository::configuration::CollectionDefinition;
use crate::schema::FieldDefinition;
use crate::schema::relations::{RelationCardinality, RelationDefinition};
use crate::source::{SourceFormat, registry::SourceRegistry};
use std::time::Instant;

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
            crate::query::options::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::query::options::SortField {
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
fn consolidation_translates_independent_local_zero_slots() {
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-local-zero-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
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
    for (file, name, records) in [
        (FileId(10), "first.jsonl", "first\nsecond\n"),
        (FileId(20), "second.jsonl", "third\nfourth\n"),
    ] {
        let path = root.join(name);
        std::fs::write(&path, records).unwrap();
        let input = SourceFileInput {
            id: file,
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
    let first_ids = shards[0]
        .records
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    let second_ids = shards[1]
        .records
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    let refs = shards.iter().collect::<Vec<_>>();
    let consolidated = IndexedCollectionSegment::consolidate(&refs, 4).unwrap();
    let first_active = crate::inventory::ActiveRecords::all(2).unwrap();
    let second_active = crate::inventory::ActiveRecords::all(2)
        .unwrap()
        .retired(&[RecordSlot(0)])
        .unwrap();
    let active = consolidated
        .translate_active(&[&first_active, &second_active])
        .unwrap();
    assert_eq!(consolidated.offsets, [0, 2]);
    assert_eq!(active.len(), 3);
    assert!(active.contains(RecordSlot(0)));
    assert!(active.contains(RecordSlot(1)));
    assert!(!active.contains(RecordSlot(2)));
    assert!(active.contains(RecordSlot(3)));
    assert_eq!(consolidated.index.records[0].id, first_ids[0]);
    assert_eq!(consolidated.index.records[1].id, first_ids[1]);
    assert_eq!(consolidated.index.records[2].id, second_ids[0]);
    assert_eq!(consolidated.index.records[3].id, second_ids[1]);
    assert_eq!(
        consolidated.index.file_records(FileId(10)).unwrap(),
        [RecordSlot(0), RecordSlot(1)]
    );
    assert_eq!(
        consolidated.index.file_records(FileId(20)).unwrap(),
        [RecordSlot(2), RecordSlot(3)]
    );
}
