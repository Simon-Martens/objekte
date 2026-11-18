use super::projection::{SlotMap, project_bitmap, project_inference, project_sorting};
use super::*;
use crate::construction::batch::SourceFileInput;
use crate::construction::file_preparation::{FilePreparationBinding, prepare_file_build};
use crate::construction::segment_admission::SegmentAdmission;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::inventory::{DiagnosticRef, FileStatus, SourceRevision};
use crate::model::{CollectionId, FieldPath, LogicalType};
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::CollectionDefinition;
use crate::source::{SourceFormat, SourceRegistry};
use crate::workload::BigramAlphabet;

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
    let scattered_inference = all
        .project_inference_for_partitions(&routing, &domains)
        .unwrap();
    for (part, sorting) in scattered.iter().enumerate() {
        let slot_map = SlotMap::Partition {
            routing: &routing,
            part,
        };
        assert_eq!(
            domains[part].logical,
            project_bitmap(&all.logical_records, &slot_map).unwrap()
        );
        assert_eq!(
            domains[part].empty_array,
            project_bitmap(&all.empty_array_records, &slot_map).unwrap()
        );
        let records = routing.selected[part]
            .iter()
            .enumerate()
            .map(|(new, &old)| {
                let mut record = all.records[old as usize].clone();
                record.slot = RecordSlot(new as u32);
                record
            })
            .collect::<Vec<_>>();
        let reference_inference = project_inference(
            &all.inference,
            &slot_map,
            &domains[part].logical,
            records.len(),
            slot_map.bounds().unwrap(),
        )
        .unwrap();
        let actual_inference = scattered_inference[part].as_ref().unwrap();
        let expected_inference = reference_inference.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&actual_inference.report).unwrap(),
            serde_json::to_value(&expected_inference.report).unwrap(),
        );
        assert_eq!(
            actual_inference.empty_selections,
            expected_inference.empty_selections
        );
        assert_eq!(
            actual_inference.short_string_coverage,
            expected_inference.short_string_coverage
        );
        assert_eq!(
            actual_inference.indexes.len(),
            expected_inference.indexes.len()
        );
        for (path, actual) in &actual_inference.indexes {
            let expected = &expected_inference.indexes[path];
            assert_eq!(actual.presence, expected.presence);
            assert_eq!(actual.empty_selections, expected.empty_selections);
            assert_eq!(actual.nulls, expected.nulls);
            assert_eq!(actual.values, expected.values);
            assert_eq!(actual.lengths, expected.lengths);
            assert_eq!(
                actual.structural.as_ref().map(|facts| &facts.postings),
                expected.structural.as_ref().map(|facts| &facts.postings)
            );
        }
        let reference = project_sorting(&all.sorting, &slot_map, &records).unwrap();
        for (path, column) in &sorting.columns {
            let expected = &reference.columns[path];
            assert_eq!(column.ascending, expected.ascending);
            assert_eq!(column.descending, expected.descending);
        }
        for (actual, expected) in sorting.composites.iter().zip(&reference.composites) {
            assert_eq!(actual.order, expected.order);
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
