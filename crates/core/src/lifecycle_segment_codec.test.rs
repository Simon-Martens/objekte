use super::*;
use std::path::PathBuf;
use std::sync::Arc;

use crate::build::{FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build};
use crate::inference::{InferenceConfig, InferenceMode};
use crate::inventory::SourceRevision;
use crate::model::{FieldPath, LogicalType};
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, Repository, SnapshotMode};
use crate::source::{SourceFormat, SourceRegistry};
use crate::storage::LifecycleSegmentCache;
use crate::workload::BigramAlphabet;

fn encode_base_metadata_segment(segment: &IndexedCollectionSegment) -> Vec<u8> {
    encode_segment(segment, &mut |entry| Ok(entry.id.0)).unwrap()
}

fn fixture() -> (
    PathBuf,
    Arc<Repository>,
    FileCatalog,
    IndexedCollectionSegment,
) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-segment-codec-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/sample.jsonl");
    std::fs::write(
        &path,
        b"{\"n\":1,\"ref\":\"alpha\"}\n{\"n\":2,\"ref\":\"beta\"}\n{bad}\n",
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    for (name, kind) in [("n", LogicalType::Number), ("ref", LogicalType::Id)] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "data".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("backlinks".into()),
        })
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    let inference = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        inference,
        ..Default::default()
    };
    let repository =
        Arc::new(Repository::open_with_options(&root, vec![definition.clone()], options).unwrap());
    let catalog = FileCatalog::from_repository(Arc::clone(&repository)).unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let prepared = prepare_file_build(
        &root,
        SourceFileInput {
            id: FileId(0),
            relative_path: PathBuf::from("data/sample.jsonl").into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: repository.files().files[0].created_ns,
        },
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
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = admission
        .seal()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    (root, repository, catalog, indexed)
}

#[test]
fn roundtrip_mixed_records_relations_and_inference_without_source_file() {
    let (root, repository, catalog, indexed) = fixture();
    assert_eq!(indexed.records.len(), 3);
    assert_eq!(indexed.logical_records.len(), 2);
    assert_eq!(indexed.relations.len(), 1);
    assert!(indexed.relations[0].references().count() >= 2);
    assert!(indexed.inference.is_some());
    let bytes = encode_base_metadata_segment(&indexed);
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let active = RoaringBitmap::from_iter([0, 1, 2]);
    let restored =
        decode_segment(&bytes, &repository, CollectionId(0), &catalog, &active, &[]).unwrap();
    for record in &restored.records {
        let (lookup_id, slot) = restored
            .id_lookup
            .get_key_value(record.id.0.as_ref())
            .unwrap();
        assert_eq!(*slot, record.slot);
        assert!(Arc::ptr_eq(&lookup_id.0, &record.id.0));
    }
    let envelope = LifecycleSegmentCache {
        ordinal: 1,
        collection: CollectionId(0),
        record_count: indexed.records.len() as u32,
        active: active.clone(),
        index: bytes.clone(),
    };
    assert_eq!(
        decode_cache_segment(&envelope, &repository, &catalog, &[])
            .unwrap()
            .records
            .len(),
        3
    );
    let mut invalid_envelope = envelope;
    invalid_envelope.record_count += 1;
    assert!(decode_cache_segment(&invalid_envelope, &repository, &catalog, &[]).is_err());
    assert_eq!(restored.records.len(), indexed.records.len());
    assert_eq!(
        restored.files[0].entry.relative_path,
        indexed.files[0].entry.relative_path
    );
    assert_eq!(restored.identity_order, indexed.identity_order);
    assert_eq!(restored.logical_records, indexed.logical_records);
    assert_eq!(
        restored.relations[0].references().count(),
        indexed.relations[0].references().count()
    );
    assert_eq!(
        restored.sorting.columns[&FieldPath::parse("n").unwrap()].ascending,
        indexed.sorting.columns[&FieldPath::parse("n").unwrap()].ascending
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_corrupt_identity_posting_sort_and_relation_domains() {
    let (root, repository, catalog, mut indexed) = fixture();
    let active = RoaringBitmap::from_iter([0, 1, 2]);
    let valid_bytes = encode_base_metadata_segment(&indexed);
    let decode = |segment: &IndexedCollectionSegment| {
        let bytes = encode_base_metadata_segment(segment);
        decode_segment(&bytes, &repository, CollectionId(0), &catalog, &active, &[])
    };
    indexed.identity_order.reverse();
    assert!(decode(&indexed).err().unwrap().contains("identity order"));
    indexed.identity_order.reverse();
    let original_end = indexed.records[2].range.end;
    indexed.records[2].range.end = indexed.files[0].entry.revision.size + 1;
    assert!(decode(&indexed).err().unwrap().contains("source span"));
    indexed.records[2].range.end = original_end;
    indexed.logical_records.insert(2);
    assert!(decode(&indexed).err().unwrap().contains("logical domain"));
    indexed.logical_records.remove(2);
    indexed
        .postings
        .fields
        .get_mut(&vec!["n".into()])
        .unwrap()
        .presence
        .insert(2);
    assert!(decode(&indexed).is_err());
    indexed
        .postings
        .fields
        .get_mut(&vec!["n".into()])
        .unwrap()
        .presence
        .remove(2);
    let original_gram = indexed.raw.normal.get(&u64::MAX).cloned();
    indexed.raw.normal.entry(u64::MAX).or_default().insert(2);
    assert!(decode(&indexed).err().unwrap().contains("domain"));
    if let Some(original) = original_gram {
        indexed.raw.normal.insert(u64::MAX, original);
    } else {
        indexed.raw.normal.remove(&u64::MAX);
    }
    indexed.inference.as_mut().unwrap().report.eligible_records += 1;
    assert!(decode(&indexed).err().unwrap().contains("inference"));
    indexed.inference.as_mut().unwrap().report.eligible_records -= 1;
    indexed
        .sorting
        .columns
        .get_mut(&FieldPath::parse("n").unwrap())
        .unwrap()
        .ascending
        .reverse();
    assert!(decode(&indexed).err().unwrap().contains("sort"));
    indexed
        .sorting
        .columns
        .get_mut(&FieldPath::parse("n").unwrap())
        .unwrap()
        .ascending
        .reverse();
    indexed.relations =
        RelationContribution::for_schema(&repository.collections()[0].definition.schema);
    assert!(decode(&indexed).err().unwrap().contains("relation"));
    let retired = catalog.retired(FileId(0)).unwrap();
    assert!(
        decode_segment(
            &valid_bytes,
            &repository,
            CollectionId(0),
            &retired,
            &active,
            &[]
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_truncated_and_empty_segment_payloads() {
    let (root, repository, catalog, indexed) = fixture();
    let mut bytes = encode_base_metadata_segment(&indexed);
    bytes.pop();
    assert!(
        decode_segment(
            &bytes,
            &repository,
            CollectionId(0),
            &catalog,
            &RoaringBitmap::new(),
            &[]
        )
        .is_err()
    );
    assert!(
        decode_segment(
            &[],
            &repository,
            CollectionId(0),
            &catalog,
            &RoaringBitmap::new(),
            &[]
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_invalid_metadata_reference_and_duplicate_record_id() {
    let (root, repository, catalog, indexed) = fixture();
    let bytes = encode_base_metadata_segment(&indexed);
    let mut encoded: OwnedSegment = codec().deserialize(&bytes).unwrap();
    encoded.files[0].metadata_ref = u32::MAX;
    let invalid_ref = codec().serialize(&encoded).unwrap();
    assert!(
        decode_segment(
            &invalid_ref,
            &repository,
            CollectionId(0),
            &catalog,
            &RoaringBitmap::new(),
            &[],
        )
        .err()
        .unwrap()
        .contains("metadata reference")
    );
    encoded.files[0].metadata_ref = 0;
    encoded.records[1].id = encoded.records[0].id.clone();
    let duplicate = codec().serialize(&encoded).unwrap();
    assert!(
        decode_segment(
            &duplicate,
            &repository,
            CollectionId(0),
            &catalog,
            &RoaringBitmap::new(),
            &[],
        )
        .err()
        .unwrap()
        .contains("duplicate segment record identity")
    );
    std::fs::remove_dir_all(root).unwrap();
}
