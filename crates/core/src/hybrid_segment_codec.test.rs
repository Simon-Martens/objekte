use super::*;
use std::io::Cursor;
use std::path::PathBuf;

use crate::build::{FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build};
use crate::inference::InferenceConfig;
use crate::inventory::SourceRevision;
use crate::model::{FieldPath, FileId};
use crate::reference::{CustomBinding, FieldDefinition, Transform};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::SnapshotMode;
use crate::source::{SourceFormat, SourceRegistry};

fn fixture() -> (
    PathBuf,
    CollectionDefinition,
    RepositoryOptions,
    IndexedCollectionSegment,
) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-hybrid-codec-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/sample.jsonl");
    std::fs::write(&path,
        b"{\"n\":1,\"ref\":\"alpha\",\"text\":\"One\"}\n{\"n\":2,\"ref\":\"beta\",\"text\":\"Two\"}\n{bad}\n").unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    for (name, kind) in [
        ("n", LogicalType::Number),
        ("ref", LogicalType::Id),
        ("text", LogicalType::String),
    ] {
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
        .register_custom(
            vec!["text".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
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
            created_ns: None,
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
    (root, definition, options, indexed)
}

#[test]
fn complete_segment_roundtrips_without_source_io() {
    let (root, definition, options, segment) = fixture();
    let mut table = Vec::<FileEntry>::new();
    let output = write_segment(
        &segment,
        1,
        &definition,
        options,
        Cursor::new(Vec::new()),
        &mut |entry| {
            table.push(entry.clone());
            Ok((table.len() - 1) as u32)
        },
    )
    .unwrap();
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(output.into_inner())).unwrap());
    std::fs::remove_dir_all(&root).unwrap();
    let decoded = decode_segment_owned(
        reader,
        1,
        &definition,
        options,
        segment.collection,
        (segment.source_id.as_str(), segment.source_version),
        segment.records.len() as u32,
        &|id| table.get(id as usize),
    )
    .unwrap();
    assert_eq!(decoded.records.len(), segment.records.len());
    assert_eq!(
        decoded
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        segment
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(decoded.files.len(), segment.files.len());
    for (actual, original) in decoded.files.iter().zip(&segment.files) {
        assert_eq!(actual.entry.relative_path, original.entry.relative_path);
        assert_eq!(actual.records, original.records);
    }
    assert_eq!(decoded.logical_records, segment.logical_records);
    assert_eq!(decoded.empty_array_records, segment.empty_array_records);
    assert_eq!(
        decoded.postings.fields.keys().collect::<HashSet<_>>(),
        segment.postings.fields.keys().collect::<HashSet<_>>()
    );
    for (path, original) in &segment.postings.fields {
        let actual = &decoded.postings.fields[path];
        assert_eq!(actual.values, original.values);
        assert_eq!(actual.lengths, original.lengths);
        assert_eq!(actual.presence, original.presence);
    }
    assert_eq!(decoded.raw.normal, segment.raw.normal);
    assert_eq!(decoded.raw.skip, segment.raw.skip);
    assert_eq!(decoded.fallback_text.normal, segment.fallback_text.normal);
    assert_eq!(decoded.fallback_text.skip, segment.fallback_text.skip);
    assert_eq!(
        decoded.custom_text.keys().collect::<HashSet<_>>(),
        segment.custom_text.keys().collect::<HashSet<_>>()
    );
    for (key, original) in &segment.custom_text {
        assert_eq!(decoded.custom_text[key].normal, original.normal);
        assert_eq!(decoded.custom_text[key].skip, original.skip);
    }
    for (key, original) in &segment.custom_postings {
        let actual = &decoded.custom_postings[key];
        assert_eq!(actual.complete, original.complete);
        assert_eq!(actual.index.values, original.index.values);
        assert_eq!(actual.index.presence, original.index.presence);
    }
    assert_eq!(
        codec()
            .serialize(&decoded.inference.as_ref().unwrap().report)
            .unwrap(),
        codec()
            .serialize(&segment.inference.as_ref().unwrap().report)
            .unwrap()
    );
    assert_eq!(decoded.sorting.columns.len(), segment.sorting.columns.len());
    for (path, original) in &segment.sorting.columns {
        let actual = &decoded.sorting.columns[path];
        assert_eq!(actual.keys, original.keys);
        assert_eq!(actual.ascending, original.ascending);
        assert_eq!(actual.descending, original.descending);
        assert_eq!(actual.options, original.options);
    }
    assert_eq!(decoded.relations.len(), segment.relations.len());
    assert_eq!(
        codec().serialize(&decoded.relations).unwrap(),
        codec().serialize(&segment.relations).unwrap()
    );
}

#[test]
fn manifest_slot_count_and_source_binding_are_checked() {
    let (root, definition, options, segment) = fixture();
    let mut table = Vec::<FileEntry>::new();
    let bytes = write_segment(
        &segment,
        1,
        &definition,
        options,
        Cursor::new(Vec::new()),
        &mut |entry| {
            table.push(entry.clone());
            Ok((table.len() - 1) as u32)
        },
    )
    .unwrap()
    .into_inner();
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    assert!(
        decode_segment_owned(
            Arc::clone(&reader),
            1,
            &definition,
            options,
            segment.collection,
            (segment.source_id.as_str(), segment.source_version),
            segment.records.len() as u32 + 1,
            &|id| table.get(id as usize)
        )
        .err()
        .unwrap()
        .contains("domain")
    );
    assert!(
        decode_segment_owned(
            Arc::clone(&reader),
            2,
            &definition,
            options,
            segment.collection,
            (segment.source_id.as_str(), segment.source_version),
            segment.records.len() as u32,
            &|id| table.get(id as usize)
        )
        .is_err()
    );
    assert!(
        decode_segment_owned(
            reader,
            1,
            &definition,
            options,
            segment.collection,
            ("wrong-source", segment.source_version),
            segment.records.len() as u32,
            &|id| table.get(id as usize)
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupted_posting_payload_fails_owned_decode() {
    let (root, definition, options, segment) = fixture();
    let mut table = Vec::<FileEntry>::new();
    let mut bytes = write_segment(
        &segment,
        1,
        &definition,
        options,
        Cursor::new(Vec::new()),
        &mut |entry| {
            table.push(entry.clone());
            Ok((table.len() - 1) as u32)
        },
    )
    .unwrap()
    .into_inner();
    let payload_entry = 24 + 5 * 56;
    assert_eq!(
        u32::from_le_bytes(bytes[payload_entry..payload_entry + 4].try_into().unwrap()),
        SectionKind::PostingPayloads as u32
    );
    let offset = u64::from_le_bytes(
        bytes[payload_entry + 8..payload_entry + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[offset] ^= 1;
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    let error = decode_segment_owned(
        reader,
        1,
        &definition,
        options,
        segment.collection,
        (segment.source_id.as_str(), segment.source_version),
        segment.records.len() as u32,
        &|id| table.get(id as usize),
    )
    .err()
    .unwrap();
    assert!(error.contains("posting checksum"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
}
