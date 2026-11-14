//! Source-free reconstruction of the native query owners from packed objects.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::build::IndexedCollectionSegment;
use crate::inference::InferenceState;
use crate::inventory::SourceDiagnostic;
use crate::inventory::{
    ByteRange, CollectionGeneration, DiagnosticRef, FileEntry, FileInventory, FileRecordIndex,
    FileStatus, FileVersion, FileVersions, RecordClass, RecordEntry, SharedPath, SourceRevision,
};
use crate::lifecycle::{
    Index, IndexError, IndexRevision, PublishedIndex, PublishedSegment, ReconciliationState,
};
use crate::model::{CollectionId, FieldPath, FileId, PathRoot, RecordId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::relation::{RelationCardinality, RelationContribution, RelationDefinition};
use crate::repository::{CollectionState, Repository};
use crate::sorting::{SortCollection, SortColumn};
use crate::source::{DiagnosticCategory, SourceRegistry};

use super::catalogue::{CatalogueView, DiagnosticView, FileRowView};
use super::error::{StorageError, StorageResult};
use super::native_pack::InferenceMetadata;
use super::segment::FrozenSegment;
use super::store::OpenedStore;

/// Reconstruct one complete queryable repository from the packed catalogue and
/// run objects in an opened store. The compatibility native-state object is
/// deliberately not read.
pub fn restore_from_store(
    opened: &OpenedStore,
    definitions: Vec<crate::repository::CollectionDefinition>,
    options: crate::repository::RepositoryOptions,
    sources: SourceRegistry,
) -> Result<Index, IndexError> {
    let configuration_identity =
        crate::configuration::configuration_hash(&definitions, options, &sources)
            .map_err(IndexError::IncompatibleConfiguration)?;
    let catalogue = CatalogueView::open(opened.catalogue_backing()).map_err(storage_error)?;
    let root = manifest_root(&opened.manifest.source_root)?;
    let (entries, rows, slots) = catalogue_entries(&catalogue)?;
    let file_entries = entries.clone();
    let files = FileInventory::from_restored_entries(root.clone(), entries)
        .map_err(IndexError::InvalidPublication)?;
    let mut base = Vec::with_capacity(definitions.len());
    let mut contributions = Vec::with_capacity(definitions.len());
    for (number, definition) in definitions.iter().enumerate() {
        let collection_id = CollectionId(number as u32);
        let source = sources
            .resolve(&definition.format)
            .map_err(IndexError::IncompatibleConfiguration)?;
        let file_ids = rows
            .values()
            .filter(|row| row.collections.contains(&collection_id.0))
            .map(|row| FileId(row.file_id))
            .collect::<Vec<_>>();
        let generation = CollectionGeneration {
            collection: collection_id,
            name: definition.name.clone(),
            generation: 1,
            files: file_ids.clone(),
            records: Vec::new(),
            file_records: FileRecordIndex {
                file_count: files.files.len() as u32,
                offsets: Vec::new(),
                slots: Vec::new(),
            },
        };
        let total_file_bytes = file_ids
            .iter()
            .filter_map(|id| rows.get(&id.0))
            .map(|row| row.size)
            .sum();
        let relation_contributions = RelationContribution::for_schema(&definition.schema);
        contributions.push(relation_contributions);
        base.push(CollectionState {
            definition: definition.clone(),
            packed: None,
            source,
            generation,
            postings: PostingCollection::new(&definition.schema),
            inference: None,
            global_inference: None,
            custom_postings: HashMap::new(),
            raw: BigramIndex::new(options.bigram_alphabet),
            fallback_text: BigramIndex::new(options.bigram_alphabet),
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            custom_text: HashMap::new(),
            text_bindings: crate::index_catalog::TextBindingCatalog::new(&definition.schema),
            sorting: SortCollection {
                columns: HashMap::new(),
                composites: Vec::new(),
            },
            index_family_bytes: 0,
            id_lookup: HashMap::new(),
            identity_order: Vec::new(),
            stats: crate::repository::CollectionStats {
                file_count: file_ids.len(),
                record_count: 0,
                total_file_bytes,
            },
            relations: Vec::new(),
        });
    }
    for (number, collection_contributions) in contributions.into_iter().enumerate() {
        let resolved = collection_contributions
            .into_iter()
            .enumerate()
            .map(|(ordinal, contribution)| {
                contribution
                    .resolve(CollectionId(number as u32), ordinal as u32, &base)
                    .map_err(IndexError::InvalidPublication)
            })
            .collect::<Result<Vec<_>, _>>()?;
        base[number].relations = resolved;
    }
    let mut frozen_runs = HashMap::new();
    for run in &opened.runs {
        let frozen = Arc::new(FrozenSegment::open(opened.run_backing(run)).map_err(storage_error)?);
        frozen_runs.insert((run.reference.collection, run.reference.ordinal), frozen);
    }
    for (number, collection) in base.iter_mut().enumerate() {
        let Some(frozen) = frozen_runs.get(&(number as u32, 0)) else {
            continue;
        };
        collection.packed = Some(Arc::clone(frozen));
        collection.stats.file_count = frozen.stats().file_part_count as usize;
        collection.stats.record_count = frozen.stats().record_count as usize;
    }
    let base_fingerprints = (0..files.files.len())
        .map(|id| rows.get(&(id as u32)).and_then(|row| row.fingerprint))
        .collect::<Vec<_>>();
    let repository = Arc::new(
        Repository::from_restored_parts(
            root,
            files.files.clone(),
            base_fingerprints,
            base,
            sources,
            options,
            0,
            0,
        )
        .map_err(IndexError::InvalidPublication)?,
    );
    let catalog = crate::lifecycle_catalog::FileCatalog::restored(Arc::clone(&repository), slots)
        .map_err(IndexError::InvalidPublication)?;
    let active = vec![
        crate::inventory::ActiveRecords::all(0).map_err(IndexError::ResourceLimit)?;
        repository.collections().len()
    ];
    let mut groups = vec![Vec::new(); repository.collections().len()];
    let mut next_segment_ordinal = 1u64;
    for run in &opened.runs {
        let collection = run.reference.collection as usize;
        let frozen = frozen_runs
            .get(&(run.reference.collection, run.reference.ordinal))
            .cloned()
            .ok_or_else(|| IndexError::InvalidPublication("opened run is missing".into()))?;
        let segment = segment_from_packed(&frozen, collection, &file_entries, &definitions)?;
        let active_slots = segment_active(&frozen)?;
        next_segment_ordinal = next_segment_ordinal.max(run.reference.ordinal + 1);
        groups
            .get_mut(collection)
            .ok_or_else(|| {
                IndexError::InvalidPublication("run collection is out of bounds".into())
            })?
            .push(Arc::new(PublishedSegment {
                ordinal: run.reference.ordinal,
                index: Arc::new(segment),
                active: crate::inventory::ActiveRecords::from_bitmap(
                    frozen.stats().record_count as usize,
                    active_slots,
                )
                .map_err(IndexError::InvalidPublication)?,
                packed: Some(Arc::clone(&frozen)),
            }));
    }
    let versions = (0..catalogue.high_water() as usize)
        .map(|id| {
            rows.get(&(id as u32))
                .map_or(FileVersion::INITIAL, |row| FileVersion(row.file_version))
        })
        .collect::<Vec<_>>();
    let file_versions =
        FileVersions::from_versions(versions).map_err(IndexError::InvalidPublication)?;
    let publication_id = uuid::Uuid::parse_str(&opened.manifest.publication_id)
        .map_err(|_| IndexError::InvalidPublication("invalid publication UUID".into()))?;
    let lineage_id = uuid::Uuid::parse_str(&opened.manifest.lineage_id)
        .map_err(|_| IndexError::InvalidPublication("invalid lineage UUID".into()))?;
    let published = PublishedIndex {
        instance_id: crate::repository::next_repository_instance(),
        publication_id,
        lineage_id,
        revision: IndexRevision::INITIAL,
        repository,
        configuration_identity,
        active,
        segments: groups.into_iter().map(Arc::new).collect(),
        next_segment_ordinal,
        last_size_maintenance_revision: 0,
        catalog,
        file_versions,
        reconciliation: ReconciliationState::NotStarted,
        processed_files: rows.len(),
        processed_memberships: Vec::new(),
    };
    Ok(Index::from_published(published))
}

fn storage_error(error: StorageError) -> IndexError {
    match error {
        StorageError::Cancelled => IndexError::Cancelled,
        StorageError::Incompatible(message) | StorageError::UnsupportedFormat(message) => {
            IndexError::IncompatibleConfiguration(message)
        }
        StorageError::ResourceLimit(message) => IndexError::ResourceLimit(message),
        other => IndexError::Storage(other.to_string()),
    }
}

fn manifest_root(hex: &str) -> Result<PathBuf, IndexError> {
    // The manifest stores a variable-length native path hex-encoded.
    let bytes = decode_hex(hex)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }

    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>, IndexError> {
    if text.len() % 2 != 0 {
        return Err(IndexError::InvalidPublication(
            "invalid source root hex".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for chunk in text.as_bytes().chunks_exact(2) {
        let high = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| IndexError::InvalidPublication("invalid source root hex".into()))?;
        let low = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| IndexError::InvalidPublication("invalid source root hex".into()))?;
        bytes.push(((high << 4) | low) as u8);
    }
    Ok(bytes)
}

fn catalogue_entries(
    catalogue: &CatalogueView,
) -> Result<
    (
        Vec<FileEntry>,
        HashMap<u32, FileRowView>,
        Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>,
    ),
    IndexError,
> {
    let mut entries = Vec::new();
    let mut rows = HashMap::new();
    let mut slots = Vec::new();
    for id in 0..catalogue.high_water() {
        let Some(row) = catalogue.file(id).map_err(storage_error)? else {
            entries.push(tombstone_entry(FileId(id), None));
            slots.push(None);
            continue;
        };
        if row.tombstone {
            entries.push(tombstone_entry(FileId(id), Some(&row)));
            slots.push(None);
            continue;
        }
        let diagnostics = catalogue.diagnostics(&row).map_err(storage_error)?;
        let entry = file_entry(&row, &diagnostics)?;
        rows.insert(id, row);
        slots.push(Some((entry.clone(), None, None)));
        entries.push(entry);
    }
    // Attach fingerprints after the row map is complete.
    for (index, slot) in slots.iter_mut().enumerate() {
        if let Some((entry, fingerprint, configuration)) = slot {
            let row = rows.get(&(index as u32));
            *fingerprint = row.and_then(|row| row.fingerprint);
            let _ = (entry, configuration);
        }
    }
    Ok((entries, rows, slots))
}

fn tombstone_entry(id: FileId, row: Option<&FileRowView>) -> FileEntry {
    FileEntry {
        id,
        relative_path: row
            .and_then(|row| native_path(&row.native_path).ok())
            .map(SharedPath::from)
            .unwrap_or_else(|| SharedPath::from(format!(".__objekte_tombstone/{}", id.0))),
        content_generation: row.map_or(1, |row| row.content_generation),
        revision: SourceRevision {
            modified_ns: row.map_or(0, |row| row.modified_ns),
            size: row.map_or(0, |row| row.size),
        },
        created_ns: row.and_then(|row| row.created_ns),
        collections: row
            .map(|row| row.collections.iter().copied().map(CollectionId).collect())
            .unwrap_or_default(),
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn file_entry(row: &FileRowView, diagnostics: &[DiagnosticView]) -> Result<FileEntry, IndexError> {
    let path = native_path(&row.native_path)?;
    let diagnostics = diagnostics
        .iter()
        .map(|diagnostic| diagnostic_entry(row, diagnostic))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FileEntry {
        id: FileId(row.file_id),
        relative_path: SharedPath::from(path),
        content_generation: row.content_generation,
        revision: SourceRevision {
            modified_ns: row.modified_ns,
            size: row.size,
        },
        created_ns: row.created_ns,
        collections: row.collections.iter().copied().map(CollectionId).collect(),
        status: match row.status {
            0 => FileStatus::Valid,
            1 => FileStatus::Broken,
            2 => FileStatus::Unreadable,
            _ => return Err(IndexError::InvalidPublication("invalid file status".into())),
        },
        diagnostics,
        omitted_diagnostics: row.omitted_diagnostics,
    })
}

fn diagnostic_entry(
    row: &FileRowView,
    diagnostic: &DiagnosticView,
) -> Result<SourceDiagnostic, IndexError> {
    let category = match diagnostic.category {
        0 => DiagnosticCategory::Framing,
        1 => DiagnosticCategory::Encoding,
        2 => DiagnosticCategory::Syntax,
        3 => DiagnosticCategory::LogicalValidation,
        4 => DiagnosticCategory::ResourceLimit,
        5 => DiagnosticCategory::Io,
        _ => {
            return Err(IndexError::InvalidPublication(
                "invalid diagnostic category".into(),
            ));
        }
    };
    Ok(SourceDiagnostic {
        file: FileId(row.file_id),
        collection: row
            .collections
            .first()
            .copied()
            .map(CollectionId)
            .unwrap_or(CollectionId(0)),
        source_id: "restored".into(),
        source_version: 1,
        record: None,
        range: None,
        category,
        code: diagnostic.code.clone(),
        field_path: (!diagnostic.field_path.is_empty()).then(|| diagnostic.field_path.clone()),
        byte_offset: diagnostic.byte_offset,
        line: diagnostic.line,
        column: diagnostic.column,
        message: diagnostic.message.clone(),
    })
}

fn native_path(bytes: &[u8]) -> Result<PathBuf, IndexError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())))
    }

    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
}

fn segment_active(frozen: &FrozenSegment) -> Result<RoaringBitmap, IndexError> {
    let mut bitmap = RoaringBitmap::new();
    frozen
        .coverage()
        .active_records()
        .map_err(storage_error)?
        .union_into(&mut bitmap)
        .map_err(storage_error)?;
    Ok(bitmap)
}

fn segment_from_packed(
    frozen: &Arc<FrozenSegment>,
    collection: usize,
    file_entries: &[FileEntry],
    definitions: &[crate::repository::CollectionDefinition],
) -> Result<IndexedCollectionSegment, IndexError> {
    let stats = frozen.stats();
    let records_view = frozen.records();
    let mut files: Vec<crate::build::SegmentFile> = Vec::new();
    let mut file_index = HashMap::new();
    let mut records = Vec::with_capacity(stats.record_count as usize);
    let mut id_lookup = HashMap::new();
    for part_number in 0..stats.file_part_count {
        let part = records_view
            .file_part(part_number)
            .map_err(storage_error)?
            .ok_or_else(|| IndexError::InvalidPublication("packed file part missing".into()))?;
        let entry = file_entries
            .get(part.file_id as usize)
            .cloned()
            .ok_or_else(|| IndexError::InvalidPublication("packed file metadata missing".into()))?;
        let file_slot = *file_index.entry(FileId(part.file_id)).or_insert_with(|| {
            files.push(crate::build::SegmentFile {
                entry: entry.clone(),
                records: Vec::new(),
            });
            files.len() - 1
        });
        for local in part.local_record_start..part.local_record_start + part.record_count {
            let slot = part.segment_slot_start + local;
            let row = records_view
                .record(slot)
                .map_err(storage_error)?
                .ok_or_else(|| IndexError::InvalidPublication("packed record missing".into()))?;
            let record = RecordEntry {
                slot: RecordSlot(slot),
                id: RecordId(row.logical_id.into()),
                file: FileId(part.file_id),
                range: ByteRange {
                    start: row.byte_start,
                    end: row.byte_end,
                },
                revision: entry.revision,
                class: match row.record_class {
                    0 => RecordClass::Logical,
                    1 => RecordClass::RawFallback,
                    _ => {
                        return Err(IndexError::InvalidPublication(
                            "invalid packed record class".into(),
                        ));
                    }
                },
                diagnostic_ref: DiagnosticRef(row.diagnostic_ref),
            };
            id_lookup.insert(record.id.clone(), record.slot);
            files[file_slot].records.push(record.slot);
            records.push(record);
        }
    }
    let (posting_collection, inference, custom_postings) = posting_families(frozen)?;
    // Bigram postings stay in the mapped packed section. Only small binding
    // descriptors are retained so status checks and packed `BigramAccess` can
    // work without rebuilding native gram dictionaries at Load.
    let (raw, fallback_text) = bigram_placeholders(frozen);
    let custom_text = HashMap::new();
    let mut sorting = SortCollection {
        columns: HashMap::new(),
        composites: Vec::new(),
    };
    for column in frozen.sorting().columns() {
        let path = FieldPath {
            root: PathRoot::Data,
            segments: column.path().to_vec(),
        };
        let keys = (0..column.key_count())
            .map(|slot| column.key_at(slot))
            .collect::<StorageResult<Option<Vec<_>>>>()
            .map_err(storage_error)?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        sorting.columns.insert(
            path,
            SortColumn {
                path: FieldPath {
                    root: PathRoot::Data,
                    segments: column.path().to_vec(),
                },
                kind: column.kind().clone(),
                keys,
                ascending: column
                    .ascending_slots()
                    .map_err(storage_error)?
                    .into_iter()
                    .map(RecordSlot)
                    .collect(),
                descending: column
                    .descending_slots()
                    .map_err(storage_error)?
                    .into_iter()
                    .map(RecordSlot)
                    .collect(),
                locale: column.locale().into(),
                options: column.options(),
                collation_version: column.collation_version().into(),
            },
        );
    }
    let mut logical_records = RoaringBitmap::new();
    frozen
        .coverage()
        .logical_records()
        .map_err(storage_error)?
        .union_into(&mut logical_records)
        .map_err(storage_error)?;
    let mut empty_array_records = RoaringBitmap::new();
    frozen
        .coverage()
        .empty_arrays()
        .map_err(storage_error)?
        .union_into(&mut empty_array_records)
        .map_err(storage_error)?;
    let mut relations = Vec::new();
    for relation in frozen.relations().relations() {
        let target_collection = definitions
            .get(relation.target_collection as usize)
            .ok_or_else(|| {
                IndexError::InvalidPublication("relation target collection missing".into())
            })?;
        let mut references = Vec::new();
        for slot in 0..relation.slot_count() {
            for edge in relation.edges(slot).map_err(storage_error)? {
                references.push(crate::relation::PendingReference {
                    source: RecordSlot(slot),
                    target_id: edge.target_id,
                    positions: edge.positions,
                });
            }
        }
        relations.push(RelationContribution {
            declaration: RelationDefinition {
                path: relation.path.clone(),
                target_collection: target_collection.name.clone(),
                cardinality: if relation_cardinality(relation) {
                    RelationCardinality::One
                } else {
                    RelationCardinality::Many
                },
                reverse_alias: relation.reverse_alias.clone(),
            },
            record_count: relation.slot_count(),
            references,
            present: relation
                .present()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            nulls: relation
                .nulls()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            empty_arrays: relation
                .empty_arrays()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
            unavailable: relation
                .unavailable()
                .map_err(storage_error)?
                .ids()
                .collect::<StorageResult<RoaringBitmap>>()
                .map_err(storage_error)?,
        });
    }
    Ok(IndexedCollectionSegment {
        collection: CollectionId(collection as u32),
        source_id: stats.source_id.clone(),
        source_version: stats.source_version,
        files,
        file_index,
        records,
        id_lookup,
        identity_order: records_view
            .identity_order()
            .map_err(storage_error)?
            .into_iter()
            .map(RecordSlot)
            .collect(),
        postings: posting_collection,
        inference,
        custom_postings,
        raw,
        fallback_text,
        custom_text,
        logical_records,
        empty_array_records,
        sorting,
        relations,
        record_parses: 0,
        failed_parses: 0,
        accounting: Default::default(),
        packed: Some(Arc::clone(frozen)),
    })
}

fn relation_cardinality(relation: &crate::index_storage::relations::RelationView) -> bool {
    relation.cardinality() == 0
}

fn posting_families(
    frozen: &FrozenSegment,
) -> Result<
    (
        PostingCollection,
        Option<InferenceState>,
        HashMap<(Vec<String>, String), CustomPostingIndex>,
    ),
    IndexError,
> {
    let metadata = frozen
        .metadata()
        .map_err(storage_error)?
        .map(|bytes| {
            bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .reject_trailing_bytes()
                .deserialize::<InferenceMetadata>(bytes)
                .map_err(|error| IndexError::InvalidPublication(error.to_string()))
        })
        .transpose()?;
    // Field dictionaries and custom postings stay in the mapped packed
    // sections. `IndexCatalog` resolves declared/observed/named access through
    // `PostingAccess::Packed`; no native value dictionary is rebuilt at Load.
    let fields = HashMap::new();
    let inference = metadata.map(|metadata| InferenceState {
        report: metadata.report,
        indexes: HashMap::new(),
        empty_selections: metadata.empty_selections,
        short_string_coverage: metadata.short_string_coverage,
    });
    Ok((PostingCollection { fields }, inference, HashMap::new()))
}

fn bigram_placeholders(
    frozen: &FrozenSegment,
) -> (crate::bigram::BigramIndex, crate::bigram::BigramIndex) {
    use crate::index_storage::bigram_builder::{BigramBinding, BigramBindingKind};
    let mut raw = crate::bigram::BigramIndex::new(crate::execution_options::BigramAlphabet::Ascii);
    let mut fallback =
        crate::bigram::BigramIndex::new(crate::execution_options::BigramAlphabet::Ascii);
    for binding in frozen.bigrams().bindings() {
        let Some(view) = (match binding {
            BigramBinding::Raw => frozen.bigrams().binding(BigramBindingKind::Raw),
            BigramBinding::Fallback => frozen.bigrams().binding(BigramBindingKind::Fallback),
            BigramBinding::Custom { .. } => None,
        }) else {
            continue;
        };
        let alphabet = match view.alphabet() {
            0 => crate::execution_options::BigramAlphabet::Ascii,
            _ => crate::execution_options::BigramAlphabet::Unicode,
        };
        match binding {
            BigramBinding::Raw => {
                raw = crate::bigram::BigramIndex::new(alphabet);
                raw.record_count = view.record_count();
            }
            BigramBinding::Fallback => {
                fallback = crate::bigram::BigramIndex::new(alphabet);
                fallback.record_count = view.record_count();
            }
            BigramBinding::Custom { .. } => {}
        }
    }
    (raw, fallback)
}
