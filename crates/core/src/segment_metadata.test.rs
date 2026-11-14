use super::*;
use crate::inventory::SourceRevision;
use crate::inventory::{FileStatus, SharedPath};
use crate::model::{FieldPath, LogicalType, Value};
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::CollectionDefinition;
use crate::sorting::{SortCollection, SortColumn, SortKey, SortOptions};
use crate::source::SourceFormat;
use std::path::PathBuf;
use std::sync::Arc;
use std::io::Cursor as IoCursor;
use crate::segment_format::{SectionKind, SegmentReader, SegmentWriter};

impl SegmentMetadataRefs for Vec<FileEntry> {
    fn entry_for_ref(&self, reference: u32) -> Result<Option<FileEntry>, String> {
        Ok(self.get(reference as usize).cloned())
    }

    fn file_id_for_ref(&self, reference: u32) -> Option<FileId> {
        self.get(reference as usize).map(|entry| entry.id)
    }

    fn revision_for_ref(&self, reference: u32) -> Option<SourceRevision> {
        self.get(reference as usize).map(|entry| entry.revision)
    }

    fn contains_collection_for_ref(&self, reference: u32, collection: CollectionId) -> Option<bool> {
        self.get(reference as usize).map(|entry| entry.collections.contains(&collection))
    }
}

fn encode_fixture(files: &[SegmentFile], records: &[RecordEntry]) -> (Vec<u8>, Vec<FileEntry>) {
    let mut catalog = Vec::new();
    let bytes = encode_records(CollectionId(0), "json", 1, files, records, &mut |entry| {
        let reference = catalog.len() as u32;
        catalog.push(entry.clone());
        Ok(reference)
    })
    .unwrap();
    (bytes, catalog)
}

fn decode_fixture(bytes: &[u8], catalog: &[FileEntry]) -> Result<CheckedRecordSlots, String> {
    decode_record_slots(bytes, CollectionId(0), ("json", 1), &|reference| {
        catalog.get(reference as usize)
    })
}

fn fixture() -> (Vec<SegmentFile>, Vec<RecordEntry>) {
    let collection = CollectionId(0);
    let file_id = FileId(7);
    let revision = SourceRevision {
        modified_ns: 123,
        size: 128,
    };
    let entry = FileEntry {
        id: file_id,
        relative_path: SharedPath::from("docs/a.json"),
        revision,
        created_ns: Some(111),
        collections: vec![collection],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    let records = vec![
        RecordEntry {
            slot: RecordSlot(0),
            id: RecordId::intrinsic("a").unwrap(),
            file: file_id,
            range: ByteRange { start: 1, end: 20 },
            revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        },
        RecordEntry {
            slot: RecordSlot(1),
            id: RecordId::fallback(
                collection,
                "json",
                1,
                &entry.relative_path,
                file_id,
                revision.modified_ns,
                20,
                80,
            ),
            file: file_id,
            range: ByteRange { start: 20, end: 80 },
            revision,
            class: RecordClass::RawFallback,
            diagnostic_ref: DiagnosticRef(0),
        },
    ];
    (
        vec![SegmentFile {
            entry,
            records: vec![RecordSlot(0), RecordSlot(1)],
        }],
        records,
    )
}

fn definition() -> CollectionDefinition {
    CollectionDefinition::folder("files", "data", SourceFormat::Json)
}

#[test]
fn record_roundtrip_restores_file_slots_domains_and_locators() {
    let (files, records) = fixture();
    let (bytes, catalog) = encode_fixture(&files, &records);
    assert!(bytes.len() < 400);
    let opened = decode_fixture(&bytes, &catalog).unwrap();
    assert_eq!(opened.slot_count(), 2);
    assert_eq!(opened.files.len(), 1);
    assert_eq!(opened.files[0].records, vec![RecordSlot(0), RecordSlot(1)]);
    assert_eq!(opened.records[0].range, records[0].range);
    assert_eq!(opened.records[1].revision, files[0].entry.revision);
    assert_eq!(opened.id_lookup[&records[0].id], RecordSlot(0));
    assert!(opened.logical_records.contains(0));
    assert!(opened.fallback_records.contains(1));
    assert!(!opened.logical_records.contains(1));
}

#[test]
fn record_decoder_rejects_identity_mismatch_truncation_and_trailing_bytes() {
    let (files, records) = fixture();
    let (bytes, catalog) = encode_fixture(&files, &records);
    let resolve = |reference| catalog.get(reference as usize);
    assert!(decode_record_slots(&bytes, CollectionId(1), ("json", 1), &resolve).is_err());
    assert!(decode_record_slots(&bytes, CollectionId(0), ("json", 2), &resolve).is_err());
    for length in [0, 12, bytes.len() - 1] {
        assert!(decode_fixture(&bytes[..length], &catalog).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode_fixture(&trailing, &catalog).is_err());
    assert!(decode_fixture(&bytes, &[]).is_err());
    let mut bad_reference = bytes.clone();
    let layout = RecordSlotsLayout::parse(&bytes, CollectionId(0), ("json", 1)).unwrap();
    bad_reference[layout.file_start + 4..layout.file_start + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_fixture(&bad_reference, &catalog).is_err());
}

#[test]
fn direct_record_reader_uses_fixed_rows_membership_and_persisted_domains() {
    let (files, records) = fixture();
    let (bytes, catalog) = encode_fixture(&files, &records);
    let mut writer = SegmentWriter::new(IoCursor::new(Vec::new()), 1).unwrap();
    writer.write_section(SectionKind::RecordSlots, |out| {
        out.write_all(&bytes).map_err(|error| error.to_string())
    }).unwrap();
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap());
    let opened = PersistedRecordSlots::open(reader, CollectionId(0), ("json", 1), Arc::new(catalog)).unwrap();
    assert_eq!(opened.record_count(), 2);
    assert_eq!(opened.file_count(), 1);
    assert_eq!(opened.record_id(RecordSlot(0)).unwrap(), Some("a"));
    assert_eq!(opened.record(RecordSlot(1)).unwrap().unwrap().id, records[1].id);
    assert_eq!(opened.file_records(FileId(7)).unwrap(), Some(vec![RecordSlot(0), RecordSlot(1)]));
    assert_eq!(opened.file_records(FileId(8)).unwrap(), None);
    assert_eq!(opened.file_entry(FileId(7)).unwrap().unwrap().relative_path, files[0].entry.relative_path);
    assert_eq!(opened.lookup_id(&[RecordSlot(0), RecordSlot(1)], "a").unwrap(), Some(RecordSlot(0)));
    assert_eq!(opened.lookup_id(&[RecordSlot(0), RecordSlot(1)], "missing").unwrap(), None);
    assert!(opened.logical_records().contains(0));
    assert!(opened.fallback_records().contains(1));
}

#[test]
fn configured_sort_column_keeps_collation_keys_and_checks_permutation() {
    let (files, records) = fixture();
    let (record_bytes, catalog) = encode_fixture(&files, &records);
    let checked = decode_fixture(&record_bytes, &catalog).unwrap();
    let path = FieldPath::parse("name").unwrap();
    let mut definition = definition();
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition.sort_fields.push(path.clone());
    let column = SortColumn {
        path: path.clone(),
        kind: LogicalType::String,
        keys: vec![
            SortKey::String(b"A".to_vec()),
            SortKey::String(b"Z".to_vec()),
        ],
        ascending: vec![RecordSlot(0), RecordSlot(1)],
        descending: vec![RecordSlot(1), RecordSlot(0)],
        locale: definition.sort_locale.clone(),
        options: SortOptions::default(),
        collation_version: COLLATION_VERSION.into(),
    };
    let mut sorting = SortCollection {
        columns: HashMap::from([(path.clone(), column)]),
        composites: Vec::new(),
    };
    let bytes = encode_sort_data(&sorting, &records).unwrap();
    let restored = decode_sort_data(&bytes, &checked, &definition).unwrap();
    assert_eq!(restored.columns[&path].keys, sorting.columns[&path].keys);
    sorting.columns.get_mut(&path).unwrap().ascending.reverse();
    assert!(encode_sort_data(&sorting, &records).is_err());
    let mut wrong_binding = definition.clone();
    wrong_binding.sort_fields.clear();
    assert!(decode_sort_data(&bytes, &checked, &wrong_binding).is_err());
}

#[test]
fn relation_section_checks_fallback_domain_and_declaration() {
    let (files, records) = fixture();
    let (record_bytes, catalog) = encode_fixture(&files, &records);
    let checked = decode_fixture(&record_bytes, &catalog).unwrap();
    let mut definition = definition();
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["ref".into()],
            kind: LogicalType::Id,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "files".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    let mut relations = RelationContribution::for_schema(&definition.schema);
    relations[0]
        .add_logical(RecordSlot(0), &Value::Null)
        .unwrap();
    relations[0].add_unavailable(RecordSlot(1)).unwrap();
    let order = vec![RecordSlot(0), RecordSlot(1)];
    let bytes = encode_relation_identity(&relations, &records, &order).unwrap();
    let decoded = decode_relation_identity(&bytes, &checked, &definition).unwrap();
    assert_eq!(decoded.relations.len(), 1);
    assert!(decoded.relations[0].unavailable().contains(1));
    assert!(decode_relation_identity(&bytes, &checked, &super::tests::definition()).is_err());
    let mut invalid = RelationContribution::for_schema(&definition.schema);
    invalid[0].add_logical(RecordSlot(0), &Value::Null).unwrap();
    invalid[0].add_logical(RecordSlot(1), &Value::Null).unwrap();
    assert!(encode_relation_identity(&invalid, &records, &order).is_err());
}

#[test]
fn record_encoder_rejects_duplicate_ids_spans_and_fallback_mismatch() {
    let (files, mut records) = fixture();
    records[1].id = records[0].id.clone();
    assert!(encode_records(CollectionId(0), "json", 1, &files, &records, &mut |_| Ok(0)).is_err());
    records[1].id = RecordId::intrinsic("wrong").unwrap();
    assert!(encode_records(CollectionId(0), "json", 1, &files, &records, &mut |_| Ok(0)).is_err());
    records[1].id = RecordId::fallback(
        CollectionId(0),
        "json",
        1,
        &files[0].entry.relative_path,
        FileId(7),
        123,
        20,
        80,
    );
    records[1].range.end = 129;
    assert!(encode_records(CollectionId(0), "json", 1, &files, &records, &mut |_| Ok(0)).is_err());
}

#[test]
fn sort_and_relation_sections_roundtrip_without_source_access() {
    let (files, records) = fixture();
    let (record_bytes, catalog) = encode_fixture(&files, &records);
    let checked = decode_fixture(&record_bytes, &catalog).unwrap();
    let sorting = SortCollection {
        columns: HashMap::new(),
        composites: Vec::new(),
    };
    let sort_bytes = encode_sort_data(&sorting, &records).unwrap();
    let decoded_sort = decode_sort_data(&sort_bytes, &checked, &definition()).unwrap();
    assert!(decoded_sort.columns.is_empty());
    let order = vec![RecordSlot(0), RecordSlot(1)];
    let relation_bytes = encode_relation_identity(&[], &records, &order).unwrap();
    let decoded_relation =
        decode_relation_identity(&relation_bytes, &checked, &definition()).unwrap();
    assert_eq!(decoded_relation.identity_order, order);
    assert!(decoded_relation.relations.is_empty());
    assert!(encode_relation_identity(&[], &records, &[RecordSlot(0), RecordSlot(0)]).is_err());
    assert!(
        decode_relation_identity(
            &relation_bytes[..relation_bytes.len() - 1],
            &checked,
            &definition()
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn record_section_preserves_non_utf8_native_paths() {
    use std::os::unix::ffi::OsStrExt;
    let (mut files, mut records) = fixture();
    files[0].entry.relative_path = SharedPath::from(PathBuf::from(std::ffi::OsStr::from_bytes(
        b"docs/\xff.json",
    )));
    records[1].id = RecordId::fallback(
        CollectionId(0),
        "json",
        1,
        &files[0].entry.relative_path,
        FileId(7),
        123,
        20,
        80,
    );
    let (bytes, catalog) = encode_fixture(&files, &records);
    let opened = decode_fixture(&bytes, &catalog).unwrap();
    assert_eq!(
        opened.files[0].entry.relative_path,
        files[0].entry.relative_path
    );
}
