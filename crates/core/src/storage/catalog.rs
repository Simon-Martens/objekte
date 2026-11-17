use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;
use rkyv::{Archive, Deserialize as ArchiveDeserialize, Serialize as ArchiveSerialize};

use crate::bigram::StoredBigramLeafRef;
use crate::inventory::{FileStatus, RecordClass};
use crate::posting::StoredPostingLeafRef;
use crate::source::DiagnosticCategory;
use crate::storage::format::sha256;
use crate::storage::model::StorageSnapshot;

pub(crate) const FILE_CATALOG_OBJECT_KIND: u16 = 12;
pub(crate) const RECORD_CATALOG_OBJECT_KIND: u16 = 13;
pub(crate) const INDEX_CATALOG_OBJECT_KIND: u16 = 14;
pub(crate) const LEAF_CATALOG_OBJECT_KIND: u16 = 15;

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredEagerCatalogRef {
    pub kind: u16,
    pub digest: [u8; 32],
    pub length: u64,
    pub rows: u64,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct EagerFileCatalog {
    pub rows: Vec<StoredEagerFileRow>,
    pub by_id: HashMap<u32, u32>,
    pub by_path: BTreeMap<Vec<u8>, u32>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredEagerFileRow {
    pub id: u32,
    pub path: Vec<u8>,
    pub modified_ns: i128,
    pub size: u64,
    pub created_ns: Option<i128>,
    pub collections: Vec<u32>,
    pub status: u8,
    pub diagnostics: Vec<StoredEagerDiagnostic>,
    pub omitted_diagnostics: u32,
    pub fingerprint: Option<[u8; 32]>,
    pub configuration: Option<[u8; 32]>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredEagerDiagnostic {
    pub file: u32,
    pub collection: u32,
    pub source_id: String,
    pub source_version: u32,
    pub record: Option<u32>,
    pub range_start: Option<u64>,
    pub range_end: Option<u64>,
    pub category: u8,
    pub code: String,
    pub field_path: Option<Vec<String>>,
    pub byte_offset: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct EagerRecordCatalog {
    pub groups: Vec<StoredEagerRecordGroup>,
    pub lookup: Vec<StoredRecordLookupRow>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredEagerRecordGroup {
    pub collection: u32,
    pub segment: u64,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<StoredEagerFileMembership>,
    pub records: Vec<StoredEagerRecordRow>,
    pub active: Vec<u8>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredEagerFileMembership {
    pub file: u32,
    pub records: Vec<u32>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize, Clone)]
pub(crate) struct StoredEagerRecordRow {
    pub slot: u32,
    pub id: String,
    pub file: u32,
    pub range_start: u64,
    pub range_end: u64,
    pub modified_ns: i128,
    pub size: u64,
    pub class: u8,
    pub diagnostic_ref: u32,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize, Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct StoredRecordLookupKey {
    pub collection: u32,
    pub id: String,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StoredRecordAddress {
    pub segment: u64,
    pub slot: u32,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredRecordLookupRow {
    pub collection: u32,
    pub id: String,
    pub address: StoredRecordAddress,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct EagerIndexCatalog {
    pub descriptors: Vec<StoredIndexDescriptor>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredIndexDescriptor {
    pub identity: String,
    pub collection: u32,
    pub segment: Option<u64>,
    pub family: u8,
    pub path: String,
    pub binding: String,
    pub complete: bool,
    pub value_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u64,
    pub dictionary_pack: Option<[u8; 32]>,
    pub dictionary_length: u64,
    pub frame_pack: Option<[u8; 32]>,
    pub frame_length: u64,
    pub leaf_group: Option<u32>,
    pub leaf_rows: u64,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct EagerLeafCatalog {
    pub groups: Vec<StoredLeafGroupDescriptor>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize, Clone, Debug)]
pub(crate) struct StoredLeafGroupDescriptor {
    pub identity: String,
    pub pack_digest: [u8; 32],
    pub pack_length: u64,
    pub codec: u8,
    pub rows: Vec<StoredLeafRow>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize, Clone, Debug)]
pub(crate) struct StoredLeafRow {
    pub offset: u64,
    pub length: u64,
    pub cardinality: u64,
    pub decoded_bytes_estimate: u64,
    pub digest: [u8; 32],
}

impl From<StoredPostingLeafRef> for StoredLeafRow {
    fn from(reference: StoredPostingLeafRef) -> Self {
        Self {
            offset: reference.offset,
            length: reference.length,
            cardinality: reference.cardinality,
            decoded_bytes_estimate: reference.decoded_bytes_estimate,
            digest: reference.digest,
        }
    }
}

impl From<StoredBigramLeafRef> for StoredLeafRow {
    fn from(reference: StoredBigramLeafRef) -> Self {
        Self {
            offset: reference.offset,
            length: reference.length,
            cardinality: reference.cardinality,
            decoded_bytes_estimate: reference.decoded_bytes_estimate,
            digest: reference.digest,
        }
    }
}

pub(crate) struct EncodedEagerCatalogs {
    pub file: ([u8; 32], Vec<u8>),
    pub record: ([u8; 32], Vec<u8>),
    pub index: ([u8; 32], Vec<u8>),
    pub leaf: ([u8; 32], Vec<u8>),
    file_rows: u64,
    record_rows: u64,
    index_rows: u64,
    leaf_rows: u64,
}

pub(crate) struct RestoredEagerCatalogs {
    pub files: std::sync::Arc<crate::archive_storage::ValidatedArchive<EagerFileCatalog>>,
    pub records: std::sync::Arc<crate::archive_storage::ValidatedArchive<EagerRecordCatalog>>,
    pub indexes: std::sync::Arc<crate::archive_storage::ValidatedArchive<EagerIndexCatalog>>,
    pub leaves: std::sync::Arc<crate::archive_storage::ValidatedArchive<EagerLeafCatalog>>,
    pub file_rows: u64,
    pub record_rows: u64,
    pub index_rows: u64,
    pub leaf_rows: u64,
    pub leaf_groups: u64,
}

impl EncodedEagerCatalogs {
    pub(crate) fn references(&self) -> Vec<StoredEagerCatalogRef> {
        [
            (FILE_CATALOG_OBJECT_KIND, &self.file, self.file_rows),
            (RECORD_CATALOG_OBJECT_KIND, &self.record, self.record_rows),
            (INDEX_CATALOG_OBJECT_KIND, &self.index, self.index_rows),
            (LEAF_CATALOG_OBJECT_KIND, &self.leaf, self.leaf_rows),
        ]
        .into_iter()
        .map(|(kind, (digest, bytes), rows)| StoredEagerCatalogRef {
            kind,
            digest: *digest,
            length: bytes.len() as u64,
            rows,
        })
        .collect()
    }

    pub(crate) fn into_objects(self) -> Vec<([u8; 32], Vec<u8>)> {
        vec![self.file, self.record, self.index, self.leaf]
    }
}

pub(crate) fn build(
    snapshot: &StorageSnapshot,
    leaf_groups: Vec<StoredLeafGroupDescriptor>,
) -> Result<EncodedEagerCatalogs, String> {
    let file = build_file_catalog(snapshot)?;
    let record = build_record_catalog(snapshot)?;
    let leaf = EagerLeafCatalog {
        groups: leaf_groups,
    };
    validate_leaf_catalog(&leaf)?;
    let index = EagerIndexCatalog {
        descriptors: build_index_descriptors(snapshot, &leaf)?,
    };
    validate_cross_catalog(&file, &record, &index, &leaf)?;
    let file_rows = file.rows.len() as u64;
    let record_rows = record
        .groups
        .iter()
        .map(|group| group.records.len() as u64)
        .sum();
    let index_rows = index.descriptors.len() as u64;
    let leaf_rows = leaf
        .groups
        .iter()
        .map(|group| group.rows.len() as u64)
        .sum();
    Ok(EncodedEagerCatalogs {
        file: encode(FILE_CATALOG_OBJECT_KIND, &file)?,
        record: encode(RECORD_CATALOG_OBJECT_KIND, &record)?,
        index: encode(INDEX_CATALOG_OBJECT_KIND, &index)?,
        leaf: encode(LEAF_CATALOG_OBJECT_KIND, &leaf)?,
        file_rows,
        record_rows,
        index_rows,
        leaf_rows,
    })
}

fn encode<T>(kind: u16, value: &T) -> Result<([u8; 32], Vec<u8>), String>
where
    T: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    let body = rkyv::to_bytes::<rkyv::rancor::Error>(value).map_err(|error| error.to_string())?;
    let object = crate::storage::format::envelope(kind, &body);
    Ok((sha256(&object), object))
}

fn build_file_catalog(snapshot: &StorageSnapshot) -> Result<EagerFileCatalog, String> {
    let mut entries = snapshot
        .files
        .files
        .iter()
        .cloned()
        .map(|entry| (entry.id.0, entry))
        .collect::<BTreeMap<_, _>>();
    for entry in snapshot
        .segments
        .iter()
        .flatten()
        .flat_map(|published| published.segment.files.iter())
        .map(|file| file.entry.clone())
    {
        entries.entry(entry.id.0).or_insert(entry);
    }
    let max_file = entries.keys().next_back().copied().unwrap_or(0);
    let placeholder_collection = snapshot
        .collections
        .first()
        .map(|collection| collection.generation.collection)
        .unwrap_or(crate::model::CollectionId(0));
    for number in 0..=max_file {
        entries
            .entry(number)
            .or_insert_with(|| crate::inventory::FileEntry {
                id: crate::model::FileId(number),
                relative_path: crate::inventory::SharedPath::from(std::path::PathBuf::from(
                    format!(".objekte/retired/{number}.json"),
                )),
                revision: crate::inventory::SourceRevision {
                    modified_ns: 0,
                    size: 0,
                },
                created_ns: None,
                collections: vec![placeholder_collection],
                status: FileStatus::Valid,
                diagnostics: Vec::new(),
                omitted_diagnostics: 0,
            });
    }
    let mut rows = Vec::with_capacity(entries.len());
    let mut by_id = HashMap::with_capacity(entries.len());
    let mut by_path = BTreeMap::new();
    for (ordinal, file) in entries.into_values().enumerate() {
        let id = u32::try_from(ordinal).map_err(|_| "too many eager file rows")?;
        if file.id.0 != id {
            return Err("eager file catalog IDs are not dense".into());
        }
        let path = native_path_bytes(file.relative_path.as_path());
        if by_id.insert(id, id).is_some() {
            return Err("eager file catalog contains duplicate identity".into());
        }
        if let Some(previous) = by_path.insert(path.clone(), id)
            && previous != id
        {
            return Err("eager file catalog contains duplicate active path identity".into());
        }
        let fingerprint = snapshot
            .catalog_slots
            .get(ordinal)
            .and_then(|slot| slot.as_ref())
            .and_then(|slot| slot.fingerprint);
        let configuration = snapshot
            .catalog_slots
            .get(ordinal)
            .and_then(|slot| slot.as_ref())
            .and_then(|slot| slot.configuration);
        rows.push(StoredEagerFileRow {
            id,
            path,
            modified_ns: file.revision.modified_ns,
            size: file.revision.size,
            created_ns: file.created_ns,
            collections: file.collections.iter().map(|value| value.0).collect(),
            status: file.status.code(),
            diagnostics: file
                .diagnostics
                .iter()
                .map(|diagnostic| StoredEagerDiagnostic {
                    file: diagnostic.file.0,
                    collection: diagnostic.collection.0,
                    source_id: diagnostic.source_id.clone(),
                    source_version: diagnostic.source_version,
                    record: diagnostic.record.map(|slot| slot.0),
                    range_start: diagnostic.range.map(|range| range.start),
                    range_end: diagnostic.range.map(|range| range.end),
                    category: diagnostic_category_code(diagnostic.category),
                    code: diagnostic.code.clone(),
                    field_path: diagnostic.field_path.clone(),
                    byte_offset: diagnostic.byte_offset,
                    line: diagnostic.line,
                    column: diagnostic.column,
                    message: diagnostic.message.clone(),
                })
                .collect(),
            omitted_diagnostics: file.omitted_diagnostics,
            fingerprint,
            configuration,
        });
    }
    Ok(EagerFileCatalog {
        rows,
        by_id,
        by_path,
    })
}

fn build_record_catalog(snapshot: &StorageSnapshot) -> Result<EagerRecordCatalog, String> {
    let mut groups = Vec::new();
    let mut lookup = Vec::new();
    let mut segment_ids = HashSet::new();
    for (collection, published_group) in snapshot.segments.iter().enumerate() {
        let collection =
            u32::try_from(collection).map_err(|_| "too many eager record collections")?;
        for published in published_group {
            let segment = &published.segment;
            if !segment_ids.insert((collection, segment.id.get())) {
                return Err("eager record catalog contains a duplicate segment route".into());
            }
            let records = segment
                .records
                .iter()
                .map(|record| {
                    let key = StoredRecordLookupKey {
                        collection,
                        id: record.id.0.as_ref().to_owned(),
                    };
                    if published.active.contains(record.slot.0) {
                        lookup.push(StoredRecordLookupRow {
                            collection: key.collection,
                            id: key.id,
                            address: StoredRecordAddress {
                                segment: segment.id.get(),
                                slot: record.slot.0,
                            },
                        });
                    }
                    Ok(StoredEagerRecordRow {
                        slot: record.slot.0,
                        id: record.id.0.as_ref().to_owned(),
                        file: record.file.0,
                        range_start: record.range.start,
                        range_end: record.range.end,
                        modified_ns: record.revision.modified_ns,
                        size: record.revision.size,
                        class: record.class.code(),
                        diagnostic_ref: record.diagnostic_ref.0,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            groups.push(StoredEagerRecordGroup {
                collection,
                segment: segment.id.get(),
                source_id: segment.source_id.clone(),
                source_version: segment.source_version,
                files: segment
                    .files
                    .iter()
                    .map(|file| StoredEagerFileMembership {
                        file: file.entry.id.0,
                        records: file.records.iter().map(|slot| slot.0).collect(),
                    })
                    .collect(),
                records,
                active: encode_roaring(&published.active)?,
            });
        }
    }
    lookup.sort_by(|left, right| {
        left.collection
            .cmp(&right.collection)
            .then_with(|| left.id.cmp(&right.id))
    });
    if lookup
        .windows(2)
        .any(|pair| pair[0].collection == pair[1].collection && pair[0].id == pair[1].id)
    {
        return Err("eager record catalog contains a duplicate active ID".into());
    }
    Ok(EagerRecordCatalog { groups, lookup })
}

fn build_index_descriptors(
    snapshot: &StorageSnapshot,
    leaf_catalog: &EagerLeafCatalog,
) -> Result<Vec<StoredIndexDescriptor>, String> {
    let leaf_groups = leaf_catalog
        .groups
        .iter()
        .enumerate()
        .map(|(ordinal, group)| {
            Ok((
                group.identity.as_str(),
                (
                    u32::try_from(ordinal).map_err(|_| "too many eager leaf groups")?,
                    group.rows.len() as u64,
                ),
            ))
        })
        .collect::<Result<HashMap<_, _>, String>>()?;
    let mut descriptors = Vec::new();
    for (collection_ordinal, stored) in snapshot.collections.iter().enumerate() {
        let collection =
            u32::try_from(collection_ordinal).map_err(|_| "too many eager index collections")?;
        add_posting_collection_descriptors(
            &mut descriptors,
            &leaf_groups,
            collection,
            None,
            &stored.postings,
        )?;
        add_bigram_descriptors(
            &mut descriptors,
            &leaf_groups,
            collection,
            None,
            "fts.raw",
            "",
            &stored.raw,
        )?;
        add_bigram_descriptors(
            &mut descriptors,
            &leaf_groups,
            collection,
            None,
            "fts.fallback",
            "",
            &stored.fallback_text,
        )?;
        for (key, bigram) in &stored.custom_text {
            add_bigram_descriptors(
                &mut descriptors,
                &leaf_groups,
                collection,
                None,
                &format!("fts.custom.{}", binding_key(key)),
                &binding_key(key),
                bigram,
            )?;
        }
        for column in stored.sorting.columns.values() {
            let (digest, length) = column.stored.map_or((None, 0), |reference| {
                (Some(reference.digest), reference.length)
            });
            descriptors.push(StoredIndexDescriptor {
                identity: format!("c{collection}:base:sort:{}", column.path.display()),
                collection,
                segment: None,
                family: 3,
                path: column.path.display(),
                binding: String::new(),
                complete: true,
                value_complete: true,
                empty_string_complete: true,
                covered_records: column.keys.len() as u64,
                dictionary_pack: None,
                dictionary_length: 0,
                frame_pack: digest,
                frame_length: length,
                leaf_group: None,
                leaf_rows: 0,
            });
        }
        for (ordinal, composite) in stored.sorting.composites.iter().enumerate() {
            let (digest, length) = composite.stored.map_or((None, 0), |reference| {
                (Some(reference.digest), reference.length)
            });
            descriptors.push(StoredIndexDescriptor {
                identity: format!("c{collection}:base:sort:composite:{ordinal}"),
                collection,
                segment: None,
                family: 3,
                path: composite
                    .fields
                    .iter()
                    .map(|field| field.path.display())
                    .collect::<Vec<_>>()
                    .join(","),
                binding: String::new(),
                complete: true,
                value_complete: true,
                empty_string_complete: true,
                covered_records: composite.order.len() as u64,
                dictionary_pack: None,
                dictionary_length: 0,
                frame_pack: digest,
                frame_length: length,
                leaf_group: None,
                leaf_rows: 0,
            });
        }
        for relation in &stored.relations {
            let reference = relation.stored_payload();
            descriptors.push(StoredIndexDescriptor {
                identity: format!(
                    "c{collection}:base:relation:{}",
                    relation.declaration.path.join(".")
                ),
                collection,
                segment: None,
                family: 4,
                path: relation.declaration.path.join("."),
                binding: relation.declaration.target_collection.clone(),
                complete: true,
                value_complete: true,
                empty_string_complete: true,
                covered_records: relation.unresolved_count(),
                dictionary_pack: None,
                dictionary_length: 0,
                frame_pack: reference.map(|value| value.digest),
                frame_length: reference.map_or(0, |value| value.length),
                leaf_group: None,
                leaf_rows: 0,
            });
        }
    }
    for (collection_ordinal, published_group) in snapshot.segments.iter().enumerate() {
        let collection =
            u32::try_from(collection_ordinal).map_err(|_| "too many eager index collections")?;
        for published in published_group {
            let segment = &published.segment;
            add_posting_collection_descriptors(
                &mut descriptors,
                &leaf_groups,
                collection,
                Some(segment.id.get()),
                &segment.postings,
            )?;
            add_bigram_descriptors(
                &mut descriptors,
                &leaf_groups,
                collection,
                Some(segment.id.get()),
                "fts.raw",
                "",
                &segment.raw,
            )?;
            add_bigram_descriptors(
                &mut descriptors,
                &leaf_groups,
                collection,
                Some(segment.id.get()),
                "fts.fallback",
                "",
                &segment.fallback_text,
            )?;
            for (key, bigram) in &segment.custom_text {
                add_bigram_descriptors(
                    &mut descriptors,
                    &leaf_groups,
                    collection,
                    Some(segment.id.get()),
                    &format!("fts.custom.{}", binding_key(key)),
                    &binding_key(key),
                    bigram,
                )?;
            }
        }
    }
    Ok(descriptors)
}

fn add_posting_collection_descriptors(
    descriptors: &mut Vec<StoredIndexDescriptor>,
    leaf_groups: &HashMap<&str, (u32, u64)>,
    collection: u32,
    segment: Option<u64>,
    postings: &crate::posting::PostingCollectionSnapshot,
) -> Result<(), String> {
    for (path, posting) in &postings.fields {
        let display_path = path.join(".");
        let collection_scope =
            segment.map_or_else(|| "base".to_owned(), |id| format!("segment{id}"));
        let identity = format!("c{collection}:{collection_scope}:posting:{display_path}");
        let (leaf_group, leaf_rows) = leaf_group_reference(leaf_groups, &identity)?;
        let dictionary = posting.payload;
        descriptors.push(StoredIndexDescriptor {
            identity,
            collection,
            segment,
            family: 1,
            path: display_path,
            binding: String::new(),
            complete: posting.value_complete || posting.length_complete,
            value_complete: posting.value_complete,
            empty_string_complete: posting.empty_string_complete,
            covered_records: posting.covered_records as u64,
            dictionary_pack: dictionary.map(|reference| reference.digest),
            dictionary_length: dictionary.map_or(0, |reference| reference.dictionary_length),
            frame_pack: None,
            frame_length: 0,
            leaf_group,
            leaf_rows,
        });
    }
    Ok(())
}

fn add_bigram_descriptors(
    descriptors: &mut Vec<StoredIndexDescriptor>,
    leaf_groups: &HashMap<&str, (u32, u64)>,
    collection: u32,
    segment: Option<u64>,
    scope: &str,
    binding: &str,
    bigram: &crate::bigram::BigramIndex,
) -> Result<(), String> {
    for representation in ["normal", "skip"] {
        let identity = storage_leaf_identity(collection as usize, segment, scope, representation);
        let (leaf_group, leaf_rows) = leaf_group_reference(leaf_groups, &identity)?;
        descriptors.push(StoredIndexDescriptor {
            identity,
            collection,
            segment,
            family: 2,
            path: binding.to_owned(),
            binding: binding.to_owned(),
            complete: true,
            value_complete: true,
            empty_string_complete: true,
            covered_records: bigram.record_count as u64,
            dictionary_pack: bigram.payload.map(|reference| reference.digest),
            dictionary_length: bigram
                .payload
                .map_or(0, |reference| reference.dictionary_length),
            frame_pack: None,
            frame_length: 0,
            leaf_group,
            leaf_rows,
        });
    }
    Ok(())
}

fn leaf_group_reference(
    leaf_groups: &HashMap<&str, (u32, u64)>,
    identity: &str,
) -> Result<(Option<u32>, u64), String> {
    let Some((ordinal, rows)) = leaf_groups.get(identity) else {
        let mut available = leaf_groups.keys().copied().collect::<Vec<_>>();
        available.sort_unstable();
        return Err(format!(
            "eager index descriptor has no leaf group: {identity}; available={available:?}"
        ));
    };
    Ok((Some(*ordinal), *rows))
}

fn validate_leaf_catalog(catalog: &EagerLeafCatalog) -> Result<(), String> {
    let mut identities = HashSet::new();
    for group in &catalog.groups {
        if group.identity.is_empty()
            || !identities.insert(group.identity.as_str())
            || group.codec != 1
        {
            return Err("eager leaf catalog group identity or codec is invalid".into());
        }
        let mut previous_end = 64u64;
        for row in &group.rows {
            if row.length == 0 {
                if row.offset != 0
                    || row.cardinality != 0
                    || row.decoded_bytes_estimate != 0
                    || row.digest != [0; 32]
                {
                    return Err("eager leaf catalog has a malformed canonical empty row".into());
                }
                continue;
            }
            let end = row
                .offset
                .checked_add(row.length)
                .ok_or("eager leaf catalog range overflow")?;
            if row.offset < previous_end
                || row.offset < 64
                || end > group.pack_length
                || row.cardinality == 0
                || row.decoded_bytes_estimate == 0
                || row.digest == [0; 32]
            {
                return Err(format!(
                    "eager leaf catalog has an invalid or overlapping leaf range for {}: offset={} end={} previous_end={} pack_length={}",
                    group.identity, row.offset, end, previous_end, group.pack_length
                ));
            }
            previous_end = end;
        }
    }
    Ok(())
}

fn validate_cross_catalog(
    files: &EagerFileCatalog,
    records: &EagerRecordCatalog,
    indexes: &EagerIndexCatalog,
    leaves: &EagerLeafCatalog,
) -> Result<(), String> {
    if files.rows.len() != files.by_id.len() {
        return Err("eager file ID map does not cover its rows".into());
    }
    for (ordinal, row) in files.rows.iter().enumerate() {
        if row.id as usize != ordinal
            || files.by_id.get(&row.id) != Some(&(ordinal as u32))
            || files.by_path.get(&row.path) != Some(&(ordinal as u32))
            || row.status > FileStatus::Unreadable.code()
        {
            return Err("eager file catalog row, ID map and path map disagree".into());
        }
    }
    let mut routes = HashSet::new();
    let lookup = records
        .lookup
        .iter()
        .map(|row| {
            (
                StoredRecordLookupKey {
                    collection: row.collection,
                    id: row.id.clone(),
                },
                row.address,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut active_ids = Vec::new();
    for group in &records.groups {
        if !routes.insert((group.collection, group.segment)) {
            return Err(
                "eager record catalog contains a duplicate collection/segment route".into(),
            );
        }
        let active = RoaringBitmap::deserialize_from(group.active.as_slice())
            .map_err(|error| format!("eager record active mask decode failed: {error}"))?;
        let mut owned = vec![false; group.records.len()];
        let mut group_files = HashSet::new();
        for membership in &group.files {
            if !group_files.insert(membership.file) {
                return Err("eager record catalog contains duplicate file membership".into());
            }
            for slot in &membership.records {
                let ordinal = *slot as usize;
                let owned = owned
                    .get_mut(ordinal)
                    .ok_or("eager record membership references a missing slot")?;
                if *owned {
                    return Err("eager record catalog contains duplicate record ownership".into());
                }
                *owned = true;
                let record = &group.records[ordinal];
                if record.file != membership.file {
                    return Err("eager record and file membership disagree".into());
                }
            }
        }
        if owned
            .iter()
            .enumerate()
            .any(|(slot, owned)| !owned && active.contains(slot as u32))
        {
            return Err("eager active record is missing file ownership".into());
        }
        for (ordinal, record) in group.records.iter().enumerate() {
            if record.slot as usize != ordinal
                || record.range_start > record.range_end
                || record.range_end > record.size
                || (active.contains(record.slot) && !group_files.contains(&record.file))
                || files.by_id.get(&record.file).is_none()
                || record.class > RecordClass::RawFallback.code()
            {
                return Err(format!(
                    "eager record catalog row or file route is invalid: collection={} segment={} slot={} file={} range={}..{} size={} class={} active={} member={} file_exists={}",
                    group.collection,
                    group.segment,
                    record.slot,
                    record.file,
                    record.range_start,
                    record.range_end,
                    record.size,
                    record.class,
                    active.contains(record.slot),
                    group_files.contains(&record.file),
                    files.by_id.get(&record.file).is_some()
                ));
            }
            if active.contains(record.slot) {
                let key = StoredRecordLookupKey {
                    collection: group.collection,
                    id: record.id.clone(),
                };
                let expected = StoredRecordAddress {
                    segment: group.segment,
                    slot: record.slot,
                };
                active_ids.push(key.clone());
                if lookup.get(&key) != Some(&expected) {
                    return Err("eager record ID lookup is ambiguous or inconsistent".into());
                }
            }
        }
    }
    active_ids.sort_by(|left, right| {
        left.collection
            .cmp(&right.collection)
            .then_with(|| left.id.cmp(&right.id))
    });
    if active_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("eager record ID lookup contains duplicate active identities".into());
    }
    for descriptor in &indexes.descriptors {
        if descriptor.identity.is_empty() {
            return Err("eager index descriptor has no identity".into());
        }
        if let Some(group) = descriptor.leaf_group {
            let group = leaves
                .groups
                .get(group as usize)
                .ok_or("eager index descriptor references a missing leaf group")?;
            if descriptor.leaf_rows != group.rows.len() as u64 {
                return Err(
                    "eager index descriptor leaf count disagrees with the leaf catalog".into(),
                );
            }
        } else if descriptor.leaf_rows != 0 {
            return Err("eager index descriptor has rows without a leaf group".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_archived_catalogs(catalogs: &RestoredEagerCatalogs) -> Result<(), String> {
    let file = catalogs.files.root();
    let records = catalogs.records.root();
    let indexes = catalogs.indexes.root();
    let leaves = catalogs.leaves.root();
    if file.rows.len() as u64 != catalogs.file_rows
        || records
            .groups
            .iter()
            .map(|group| group.records.len() as u64)
            .sum::<u64>()
            != catalogs.record_rows
        || indexes.descriptors.len() as u64 != catalogs.index_rows
        || leaves
            .groups
            .iter()
            .map(|group| group.rows.len() as u64)
            .sum::<u64>()
            != catalogs.leaf_rows
        || leaves.groups.len() as u64 != catalogs.leaf_groups
    {
        return Err("eager catalog row counts disagree with the manifest".into());
    }
    if file.rows.len() != file.by_id.len() || file.rows.len() != file.by_path.len() {
        return Err("eager file catalog maps do not cover the row array".into());
    }
    let mut file_ids = HashSet::new();
    for (ordinal, row) in file.rows.iter().enumerate() {
        let id = row.id.to_native();
        if id as usize != ordinal
            || !file_ids.insert(id)
            || file.by_id.get(&rkyv::rend::u32_le::from_native(id)) != Some(&row.id)
            || file.by_path.get(row.path.as_slice()) != Some(&row.id)
            || row.status > FileStatus::Unreadable.code()
        {
            return Err("eager file catalog row, by-ID map and ordered path map disagree".into());
        }
    }
    let mut routes = HashSet::new();
    let mut active_ids = Vec::new();
    for group in records.groups.iter() {
        let collection = group.collection.to_native();
        let segment = group.segment.to_native();
        if !routes.insert((collection, segment)) {
            return Err("eager record catalog contains a duplicate route".into());
        }
        let active = RoaringBitmap::deserialize_from(group.active.as_slice())
            .map_err(|error| format!("eager record active mask decode failed: {error}"))?;
        let mut owned = vec![false; group.records.len()];
        let mut group_files = HashSet::new();
        for membership in group.files.iter() {
            let file = membership.file.to_native();
            if !group_files.insert(file) || !file_ids.contains(&file) {
                return Err("eager record file membership is duplicate or missing".into());
            }
            for slot in membership.records.iter() {
                let slot = slot.to_native();
                let owned = owned
                    .get_mut(slot as usize)
                    .ok_or("eager record membership references a missing slot")?;
                if *owned || group.records[slot as usize].file.to_native() != file {
                    return Err("eager record and file membership disagree".into());
                }
                *owned = true;
            }
        }
        if owned
            .iter()
            .enumerate()
            .any(|(slot, owned)| !owned && active.contains(slot as u32))
        {
            return Err("eager active record is missing file ownership".into());
        }
        for (ordinal, row) in group.records.iter().enumerate() {
            let slot = row.slot.to_native();
            let file = row.file.to_native();
            if slot as usize != ordinal
                || row.range_start.to_native() > row.range_end.to_native()
                || row.range_end.to_native() > row.size.to_native()
                || (active.contains(slot) && !group_files.contains(&file))
                || row.class > RecordClass::RawFallback.code()
            {
                return Err(format!(
                    "eager record catalog row or file route is invalid: collection={collection} segment={segment} slot={slot} file={file} range={}..{} size={} active={}",
                    row.range_start.to_native(),
                    row.range_end.to_native(),
                    row.size.to_native(),
                    active.contains(slot),
                ));
            }
            if active.contains(slot) {
                active_ids.push((collection, row.id.as_str().to_owned(), segment, slot));
            }
        }
    }
    active_ids.sort();
    if active_ids
        .windows(2)
        .any(|pair| pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1)
    {
        return Err("eager record catalog contains duplicate active identities".into());
    }
    if records.lookup.len() != active_ids.len() {
        return Err("eager record lookup does not cover the record rows".into());
    }
    for (row, expected) in records.lookup.iter().zip(&active_ids) {
        if row.collection.to_native() != expected.0
            || row.id.as_str() != expected.1
            || row.address.segment.to_native() != expected.2
            || row.address.slot.to_native() != expected.3
        {
            return Err("eager record lookup disagrees with record rows".into());
        }
    }
    let mut leaf_identities = HashSet::new();
    for group in leaves.groups.iter() {
        if group.identity.as_str().is_empty()
            || !leaf_identities.insert(group.identity.as_str())
            || group.codec != 1
        {
            return Err("eager leaf group identity or codec is invalid".into());
        }
        let pack_length = group.pack_length.to_native();
        let mut previous_end = 64u64;
        for row in group.rows.iter() {
            let offset = row.offset.to_native();
            let length = row.length.to_native();
            if length == 0 {
                if offset != 0
                    || row.cardinality.to_native() != 0
                    || row.decoded_bytes_estimate.to_native() != 0
                    || row.digest != [0; 32]
                {
                    return Err("eager leaf catalog has a malformed empty row".into());
                }
                continue;
            }
            let end = offset
                .checked_add(length)
                .ok_or("eager leaf catalog range overflow")?;
            if offset < previous_end
                || offset < 64
                || end > pack_length
                || row.cardinality.to_native() == 0
                || row.decoded_bytes_estimate.to_native() == 0
                || row.digest == [0; 32]
            {
                return Err("eager leaf catalog has an invalid or overlapping range".into());
            }
            previous_end = end;
        }
    }
    let mut descriptor_identities = HashSet::new();
    for descriptor in indexes.descriptors.iter() {
        if descriptor.identity.as_str().is_empty()
            || !descriptor_identities.insert(descriptor.identity.as_str())
        {
            return Err("eager index descriptor identity is empty or duplicate".into());
        }
        if let Some(group) = descriptor
            .leaf_group
            .as_ref()
            .map(|value| value.to_native())
        {
            let group = leaves
                .groups
                .get(group as usize)
                .ok_or("eager index descriptor references a missing leaf group")?;
            if descriptor.leaf_rows.to_native() != group.rows.len() as u64 {
                return Err("eager index leaf count disagrees with the leaf catalog".into());
            }
        } else if descriptor.leaf_rows.to_native() != 0 {
            return Err("eager index descriptor has rows without a leaf group".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_snapshot_binding(
    snapshot: &StorageSnapshot,
    catalogs: &RestoredEagerCatalogs,
) -> Result<(), String> {
    let files = catalogs.files.root();
    for (ordinal, file) in snapshot.files.files.iter().enumerate() {
        let row = files
            .rows
            .get(ordinal)
            .ok_or("eager file catalog does not cover the JSON file table")?;
        if row.id.to_native() != file.id.0
            || row.path.as_slice() != native_path_bytes(file.relative_path.as_path())
            || row.modified_ns.to_native() != file.revision.modified_ns
            || row.size.to_native() != file.revision.size
            || row.status != file.status.code()
        {
            return Err("eager and JSON file catalogs disagree".into());
        }
    }
    let groups = catalogs.records.root().groups.as_slice();
    let mut cursor = 0usize;
    for published_group in &snapshot.segments {
        for published in published_group {
            let group = groups
                .get(cursor)
                .ok_or("eager record catalog does not cover JSON segments")?;
            cursor += 1;
            if group.segment.to_native() != published.segment.id.get()
                || group.collection.to_native() != published.segment.collection.0
                || group.records.len() != published.segment.records.len()
            {
                return Err("eager and JSON record segment routes disagree".into());
            }
            let active = RoaringBitmap::deserialize_from(group.active.as_slice())
                .map_err(|error| format!("eager record active mask decode failed: {error}"))?;
            if active != published.active {
                return Err("eager and JSON record liveness disagree".into());
            }
            for (ordinal, (row, record)) in group
                .records
                .iter()
                .zip(&published.segment.records)
                .enumerate()
            {
                if row.slot.to_native() as usize != ordinal
                    || row.id.as_str() != record.id.0.as_ref()
                    || row.file.to_native() != record.file.0
                    || row.range_start.to_native() != record.range.start
                    || row.range_end.to_native() != record.range.end
                {
                    return Err("eager and JSON record rows disagree".into());
                }
            }
        }
    }
    if cursor != groups.len() {
        return Err("eager record catalog contains routes absent from JSON".into());
    }
    Ok(())
}

pub(crate) fn checked_from_archive<T>(
    bytes: &[u8],
    kind: u16,
) -> Result<
    (
        std::sync::Arc<crate::archive_storage::ValidatedArchive<T>>,
        u64,
    ),
    String,
>
where
    T: Archive,
    T::Archived: rkyv::Portable
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
{
    let body =
        crate::storage::format::checked_body(bytes, kind).map_err(|error| error.to_string())?;
    let mut aligned = rkyv::util::AlignedVec::<16>::with_capacity(body.len());
    aligned.extend_from_slice(body);
    let owner = crate::archive_storage::ValidatedArchive::<T>::checked_from_aligned(aligned)
        .map_err(|error| error.to_string())?;
    Ok((owner, bytes.len() as u64))
}

fn encode_roaring(bitmap: &roaring::RoaringBitmap) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(bitmap.serialized_size());
    bitmap
        .serialize_into(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn diagnostic_category_code(category: DiagnosticCategory) -> u8 {
    match category {
        DiagnosticCategory::Framing => 0,
        DiagnosticCategory::Encoding => 1,
        DiagnosticCategory::Syntax => 2,
        DiagnosticCategory::LogicalValidation => 3,
        DiagnosticCategory::ResourceLimit => 4,
        DiagnosticCategory::Io => 5,
    }
}

fn binding_key(key: &(Vec<String>, String)) -> String {
    format!("{}.{}", key.0.join("."), key.1)
}

fn native_path_bytes(path: &std::path::Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

pub(crate) fn storage_leaf_identity(
    collection: usize,
    segment: Option<u64>,
    family: &str,
    path: &str,
) -> String {
    let scope = segment.map_or_else(|| "base".to_owned(), |value| format!("segment{value}"));
    format!("c{collection}:{scope}:{family}:{path}")
}

pub(crate) fn quick_leaf_group(
    identity: String,
    pack_digest: [u8; 32],
    pack_length: u64,
    mut rows: Vec<StoredLeafRow>,
) -> StoredLeafGroupDescriptor {
    rows.sort_by_key(|row| (row.length == 0, row.offset));
    StoredLeafGroupDescriptor {
        identity,
        pack_digest,
        pack_length,
        codec: 1,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_catalogs() -> (
        EagerFileCatalog,
        EagerRecordCatalog,
        EagerIndexCatalog,
        EagerLeafCatalog,
    ) {
        let file = StoredEagerFileRow {
            id: 0,
            path: b"data/a.json".to_vec(),
            modified_ns: 1,
            size: 1,
            created_ns: None,
            collections: vec![0],
            status: FileStatus::Valid.code(),
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
            fingerprint: None,
            configuration: None,
        };
        let record = StoredEagerRecordRow {
            slot: 0,
            id: "a".into(),
            file: 0,
            range_start: 0,
            range_end: 1,
            modified_ns: 1,
            size: 1,
            class: RecordClass::Logical.code(),
            diagnostic_ref: 0,
        };
        (
            EagerFileCatalog {
                rows: vec![file],
                by_id: HashMap::from([(0, 0)]),
                by_path: BTreeMap::from([(b"data/a.json".to_vec(), 0)]),
            },
            EagerRecordCatalog {
                groups: vec![StoredEagerRecordGroup {
                    collection: 0,
                    segment: 1,
                    source_id: "json".into(),
                    source_version: 1,
                    files: vec![StoredEagerFileMembership {
                        file: 0,
                        records: vec![0],
                    }],
                    records: vec![record],
                    active: encode_roaring(&roaring::RoaringBitmap::from_iter([0])).unwrap(),
                }],
                lookup: vec![StoredRecordLookupRow {
                    collection: 0,
                    id: "a".into(),
                    address: StoredRecordAddress {
                        segment: 1,
                        slot: 0,
                    },
                }],
            },
            EagerIndexCatalog {
                descriptors: Vec::new(),
            },
            EagerLeafCatalog { groups: Vec::new() },
        )
    }

    #[test]
    fn eager_catalog_rejects_record_route_to_missing_file() {
        let (files, mut records, indexes, leaves) = valid_catalogs();
        records.groups[0].records[0].file = 7;
        records.groups[0].files[0].file = 7;
        let error = validate_cross_catalog(&files, &records, &indexes, &leaves).unwrap_err();
        assert!(error.contains("file route is invalid"), "{error}");
    }

    #[test]
    fn eager_catalog_rejects_duplicate_active_identity() {
        let (files, mut records, indexes, leaves) = valid_catalogs();
        let mut duplicate = records.groups[0].records[0].clone();
        duplicate.slot = 1;
        records.groups[0].records.push(duplicate);
        records.groups[0].files[0].records.push(1);
        records.groups[0].active =
            encode_roaring(&roaring::RoaringBitmap::from_iter([0, 1])).unwrap();
        records.lookup.push(StoredRecordLookupRow {
            collection: 0,
            id: "a".into(),
            address: StoredRecordAddress {
                segment: 1,
                slot: 1,
            },
        });
        let error = validate_cross_catalog(&files, &records, &indexes, &leaves).unwrap_err();
        assert!(error.contains("ambiguous or inconsistent"), "{error}");
    }

    #[test]
    fn eager_leaf_catalog_rejects_overlapping_ranges() {
        let row = StoredLeafRow {
            offset: 64,
            length: 16,
            cardinality: 1,
            decoded_bytes_estimate: 16,
            digest: [1; 32],
        };
        let catalog = EagerLeafCatalog {
            groups: vec![StoredLeafGroupDescriptor {
                identity: "c0:base:posting:path".into(),
                pack_digest: [2; 32],
                pack_length: 128,
                codec: 1,
                rows: vec![row.clone(), row],
            }],
        };
        assert!(validate_leaf_catalog(&catalog).is_err());
    }
}
