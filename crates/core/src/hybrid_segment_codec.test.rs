use super::*;
use std::io::Cursor;
use std::path::PathBuf;

use crate::bigram::BigramRequest;
use crate::build::{FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build};
use crate::inference::InferenceConfig;
use crate::inventory::SourceRevision;
use crate::model::{FieldPath, FileId};
use crate::query::parse_filter;
use crate::reference::{BoundExpr, bind_filter};
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
fn sealed_reopen_from_mapped_and_owned_bytes_queries_without_source() {
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
    let cache = root.join("segment.idx");
    std::fs::write(&cache, &bytes).unwrap();
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let readers = [
        Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap()),
        Arc::new(SegmentReader::from_index_file(&std::fs::File::open(&cache).unwrap()).unwrap()),
    ];
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("n >= 2").unwrap(), &definition.schema).unwrap()
    else {
        panic!("expected term");
    };
    let raw_request = BigramRequest::new("One");
    for reader in readers {
        let decoded = decode_segment_sealed(
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
        let path = vec!["n".to_owned()];
        let posting = &decoded.postings.fields[&path];
        assert!(posting.values.is_empty());
        assert!(posting.lengths.is_empty());
        assert!(posting.sealed_values().is_some());
        assert_eq!(
            posting
                .exact_set_checked(&term, &decoded.logical_records)
                .unwrap(),
            segment.postings.fields[&path].exact_set(&term, &segment.logical_records),
        );
        assert!(decoded.raw.normal.is_empty());
        assert!(decoded.raw.skip.is_empty());
        assert_eq!(
            decoded.raw.try_candidates_prepared(&raw_request).unwrap(),
            segment.raw.candidates_prepared(&raw_request),
        );
        assert!(posting.values.is_empty());
        assert!(decoded.raw.normal.is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sealed_segment_reencodes_after_move_and_retirement_without_source() {
    let (root, definition, options, segment) = fixture();
    let mut metadata = Vec::<FileEntry>::new();
    let bytes = write_segment(
        &segment,
        1,
        &definition,
        options,
        Cursor::new(Vec::new()),
        &mut |entry| {
            metadata.push(entry.clone());
            Ok((metadata.len() - 1) as u32)
        },
    )
    .unwrap()
    .into_inner();
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let reopened = decode_segment_sealed(
        Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap()),
        1,
        &definition,
        options,
        segment.collection,
        (segment.source_id.as_str(), segment.source_version),
        segment.records.len() as u32,
        &|id| metadata.get(id as usize),
    )
    .unwrap();
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("n >= 1").unwrap(), &definition.schema).unwrap()
    else {
        panic!("expected term");
    };
    let path = vec!["n".to_owned()];
    let active = RoaringBitmap::from_iter([1]);
    let before = reopened.postings.fields[&path]
        .exact_set_checked(&term, &reopened.logical_records)
        .unwrap()
        .unwrap()
        & &active;
    let gram = BigramRequest::new("One");
    let before_gram = reopened.raw.try_candidates_prepared(&gram).unwrap();
    // A catalog move changes the current path. The segment keeps its indexed
    // historical path because fallback record IDs are derived from it.
    let mut moved_current = reopened.files[0].entry.clone();
    moved_current.relative_path = PathBuf::from("data/moved.jsonl").into();
    let mut moved_metadata = Vec::<FileEntry>::new();
    let rewritten = write_segment(
        &reopened,
        2,
        &definition,
        options,
        Cursor::new(Vec::new()),
        &mut |entry| {
            moved_metadata.push(entry.clone());
            Ok((moved_metadata.len() - 1) as u32)
        },
    )
    .unwrap()
    .into_inner();
    let restored = decode_segment_sealed(
        Arc::new(SegmentReader::from_owned(Arc::from(rewritten)).unwrap()),
        2,
        &definition,
        options,
        segment.collection,
        (segment.source_id.as_str(), segment.source_version),
        segment.records.len() as u32,
        &|id| moved_metadata.get(id as usize),
    )
    .unwrap();
    assert_eq!(
        moved_current.relative_path,
        PathBuf::from("data/moved.jsonl")
    );
    assert_eq!(
        restored.files[0].entry.relative_path,
        PathBuf::from("data/sample.jsonl")
    );
    assert_eq!(
        restored.postings.fields[&path]
            .exact_set_checked(&term, &restored.logical_records)
            .unwrap()
            .unwrap()
            & &active,
        before,
    );
    assert_eq!(
        restored.raw.try_candidates_prepared(&gram).unwrap(),
        before_gram
    );
    assert!(restored.postings.fields[&path].values.is_empty());
    assert!(restored.raw.normal.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn owned_seal_releases_builder_maps_without_source_or_temp_file() {
    let (root, definition, options, segment) = fixture();
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let started = std::time::Instant::now();
    let sealed = seal_segment_owned(&segment, 1, &definition, options).unwrap();
    eprintln!("owned hybrid segment seal: {:?}", started.elapsed());
    assert!(!root.join("segment.idx").exists());
    let path = vec!["n".to_owned()];
    assert!(sealed.postings.fields[&path].values.is_empty());
    assert!(sealed.postings.fields[&path].sealed_values().is_some());
    assert!(sealed.raw.normal.is_empty());
    assert!(sealed.raw.skip.is_empty());
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("n >= 2").unwrap(), &definition.schema).unwrap()
    else {
        panic!("expected term");
    };
    assert_eq!(
        sealed.postings.fields[&path]
            .exact_set_checked(&term, &sealed.logical_records)
            .unwrap(),
        segment.postings.fields[&path].exact_set(&term, &segment.logical_records),
    );
    let gram = BigramRequest::new("One");
    assert_eq!(
        sealed.raw.try_candidates_prepared(&gram).unwrap(),
        segment.raw.candidates_prepared(&gram),
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn consuming_seal_keeps_builder_metadata_and_queries_without_source() {
    let (root, definition, options, segment) = fixture();
    let path = vec!["n".to_owned()];
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("n >= 2").unwrap(), &definition.schema).unwrap()
    else {
        panic!("expected term");
    };
    let expected = segment.postings.fields[&path].exact_set(&term, &segment.logical_records);
    let gram = BigramRequest::new("One");
    let expected_gram = segment.raw.candidates_prepared(&gram);
    let files = segment.file_index.clone();
    let ids = segment.id_lookup.clone();
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let sealed = seal_segment_consuming(segment, 1, &definition, options).unwrap();
    assert_eq!(sealed.file_index, files);
    assert_eq!(sealed.id_lookup, ids);
    assert!(sealed.postings.fields[&path].values.is_empty());
    assert!(sealed.raw.normal.is_empty());
    assert_eq!(
        sealed.postings.fields[&path]
            .exact_set_checked(&term, &sealed.logical_records).unwrap(),
        expected,
    );
    assert_eq!(sealed.raw.try_candidates_prepared(&gram).unwrap(), expected_gram);
    assert!(!root.join("segment.idx").exists());
    std::fs::remove_dir_all(root).unwrap();
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

#[test]
fn sealed_decode_defers_posting_payload_error_until_query() {
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
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes.clone())).unwrap());
    let fields: FieldSection = decode_meta(
        FIELD_MAGIC,
        required(&reader, SectionKind::FieldMetadata).unwrap(),
    )
    .unwrap();
    let field = fields
        .bindings
        .iter()
        .find(|field| field.key == PostingKey::Declared(vec!["n".into()]))
        .unwrap();
    let span = field.values.checked().unwrap();
    let dictionary = OrderedBlockDictionary::<ScalarKey>::open(
        reader,
        SectionKind::ScalarDictionaries,
        span.start,
        span.length,
    )
    .unwrap();
    let descriptor = dictionary
        .get(&ScalarKey::Number(Number::integer(1)))
        .unwrap()
        .unwrap();
    let payload_entry = 24 + 5 * 56;
    assert_eq!(
        u32::from_le_bytes(bytes[payload_entry..payload_entry + 4].try_into().unwrap()),
        SectionKind::PostingPayloads as u32
    );
    let payload_offset = u64::from_le_bytes(
        bytes[payload_entry + 8..payload_entry + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[payload_offset + descriptor.offset as usize] ^= 1;
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let decoded = decode_segment_sealed(
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
    let BoundExpr::Term(term) =
        bind_filter(&parse_filter("n = 1").unwrap(), &definition.schema).unwrap()
    else {
        panic!("expected term");
    };
    assert!(
        decoded.postings.fields[&vec!["n".into()]]
            .exact_set_checked(&term, &decoded.logical_records)
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sealed_decode_defers_bigram_payload_error_until_query() {
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
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes.clone())).unwrap());
    let texts: TextSection = decode_meta(
        TEXT_MAGIC,
        required(&reader, SectionKind::TextBindings).unwrap(),
    )
    .unwrap();
    let binding = texts
        .bindings
        .iter()
        .find(|binding| binding.key == TextKey::Raw)
        .unwrap();
    let request = BigramRequest::new("One");
    let (skip, key) = request.grams(options.bigram_alphabet)[0];
    let span = if skip { binding.skip } else { binding.normal }
        .checked()
        .unwrap();
    let dictionary = OrderedBlockDictionary::<u64>::open(
        reader,
        SectionKind::BigramDictionaries,
        span.start,
        span.length,
    )
    .unwrap();
    let descriptor = dictionary.get(&key).unwrap().unwrap();
    let payload_entry = 24 + 5 * 56;
    let payload_offset = u64::from_le_bytes(
        bytes[payload_entry + 8..payload_entry + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[payload_offset + descriptor.offset as usize] ^= 1;
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    std::fs::remove_file(root.join("data/sample.jsonl")).unwrap();
    let decoded = decode_segment_sealed(
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
    assert!(decoded.raw.try_candidates_prepared(&request).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
