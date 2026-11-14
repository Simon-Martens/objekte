//! Source-free capture and restoration of the native query representation.
//!
//! Packed segment and catalogue objects are the persistent readers. This
//! compatibility object lets the existing execution engine reopen the exact
//! published view without reading source files or replaying mutations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::build::IndexedCollectionSegment;
use crate::inventory::{ActiveRecords, FileEntry, FileVersion, FileVersions};
use crate::lifecycle::{
    Index, IndexError, IndexRevision, IndexView, PublishedIndex, PublishedSegment,
    ReconciliationState,
};
use crate::model::{CollectionId, RecordId, RecordSlot};
use crate::repository::{
    CollectionDefinition, CollectionState, GlobalInferenceSummary, Repository, RepositoryOptions,
};
use crate::source::SourceRegistry;

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
use super::format::{ObjectBuilder, ObjectKind, ObjectView, SectionKind};

/// Native state object format version.
pub const NATIVE_STATE_VERSION: u32 = 1;
/// Model contract version stored with native compatibility state.
pub const NATIVE_MODEL_VERSION: u32 = 1;


#[derive(Serialize, Deserialize)]
struct NativeCollection {
    generation: crate::inventory::CollectionGeneration,
    postings: crate::posting::PostingCollection,
    inference: Option<crate::inference::InferenceState>,
    global_inference: Option<GlobalInferenceSummary>,
    custom_postings: HashMap<(Vec<String>, String), crate::posting::CustomPostingIndex>,
    raw: crate::bigram::BigramIndex,
    fallback_text: crate::bigram::BigramIndex,
    custom_text: HashMap<(Vec<String>, String), crate::bigram::BigramIndex>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: crate::sorting::SortCollection,
    index_family_bytes: usize,
    id_lookup: Vec<(RecordId, RecordSlot)>,
    identity_order: Vec<RecordSlot>,
    stats: crate::repository::CollectionStats,
    relations: Vec<crate::relation::RelationIndex>,
}


#[derive(Serialize, Deserialize)]
struct NativeSegment {
    ordinal: u64,
    collection: u32,
    index: IndexedCollectionSegment,
    active: RoaringBitmap,
}


#[derive(Serialize, Deserialize)]
struct NativePublication {
    revision: u64,
    next_segment_ordinal: u64,
    last_size_maintenance_revision: u64,
    construction_complete: bool,
    publication_id: String,
    lineage_id: String,
    processed_files: usize,
    active: Vec<RoaringBitmap>,
    file_versions: Vec<u64>,
    catalog_slots: Vec<Option<(FileEntry, Option<[u8; 32]>, Option<[u8; 32]>)>>,
    segments: Vec<NativeSegment>,
}


/// Opaque native state written into one object.
#[derive(Serialize, Deserialize)]
pub struct NativeState {
    version: u32,
    model_version: u32,
    root: PathBuf,
    files: Vec<FileEntry>,
    base_fingerprints: Vec<Option<[u8; 32]>>,
    build_parses: u64,
    build_failed_parses: u64,
    collections: Vec<NativeCollection>,
    publication: NativePublication,
}


fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}


impl NativeState {
    /// Capture one pinned publication without touching source files.
    pub fn capture(view: &IndexView) -> Result<Self, IndexError> {
        let published = &view.0;
        let repository = view.repository();
        let mut collections = Vec::with_capacity(repository.collections().len());
        for collection in repository.collections() {
            collections.push(NativeCollection {
                generation: collection.generation.clone(),
                postings: collection.postings.clone(),
                inference: collection.inference.clone(),
                global_inference: collection.global_inference.clone(),
                custom_postings: collection.custom_postings.clone(),
                raw: collection.raw.clone(),
                fallback_text: collection.fallback_text.clone(),
                custom_text: collection.custom_text.clone(),
                logical_records: collection.logical_records.clone(),
                empty_array_records: collection.empty_array_records.clone(),
                sorting: collection.sorting.clone(),
                index_family_bytes: collection.index_family_bytes,
                id_lookup: collection
                    .id_lookup
                    .iter()
                    .map(|(id, slot)| (id.clone(), *slot))
                    .collect(),
                identity_order: collection.identity_order.clone(),
                stats: collection.stats,
                relations: collection.relations.clone(),
            });
        }
        let mut segments = Vec::new();
        for (collection, group) in published.segments.iter().enumerate() {
            for segment in group.iter() {
                segments.push(NativeSegment {
                    ordinal: segment.ordinal,
                    collection: collection as u32,
                    index: (*segment.index).clone(),
                    active: segment.active.bitmap(),
                });
            }
        }
        Ok(Self {
            version: NATIVE_STATE_VERSION,
            model_version: NATIVE_MODEL_VERSION,
            root: repository.files().root.clone(),
            files: repository.files().files.clone(),
            base_fingerprints: repository.base_fingerprints.clone(),
            build_parses: repository.build_parses,
            build_failed_parses: repository.build_failed_parses,
            collections,
            publication: NativePublication {
                revision: published.revision.0,
                next_segment_ordinal: published.next_segment_ordinal,
                last_size_maintenance_revision: published.last_size_maintenance_revision,
                construction_complete: published.catalog.construction_complete(),
                publication_id: published.publication_id.to_string(),
                lineage_id: published.lineage_id.to_string(),
                processed_files: published.processed_files,
                active: published.active.iter().map(ActiveRecords::bitmap).collect(),
                file_versions: published
                    .file_versions
                    .versions()
                    .into_iter()
                    .map(|version| version.0)
                    .collect(),
                catalog_slots: published.catalog.snapshot_slots(),
                segments,
            },
        })
    }

    /// Restore one exact published view with supplied runtime definitions.
    pub fn restore(
        self,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
    ) -> Result<Index, IndexError> {
        if self.version != NATIVE_STATE_VERSION {
            return Err(IndexError::IncompatibleConfiguration(format!(
                "native state version {}",
                self.version
            )));
        }
        if self.model_version != NATIVE_MODEL_VERSION {
            return Err(IndexError::IncompatibleConfiguration(format!(
                "model version {} differs from {}",
                self.model_version, NATIVE_MODEL_VERSION
            )));
        }
        if !self.publication.construction_complete {
            return Err(IndexError::IncompleteBuild);
        }
        if self.collections.len() != definitions.len() {
            return Err(IndexError::IncompatibleConfiguration(
                "collection count differs from the saved index".into(),
            ));
        }
        let configuration_identity =
            crate::configuration::configuration_hash(&definitions, options, &sources)
                .map_err(IndexError::IncompatibleConfiguration)?;
        let mut restored = Vec::with_capacity(definitions.len());
        for (number, (state, definition)) in self
            .collections
            .into_iter()
            .zip(definitions.iter())
            .enumerate()
        {
            if state.generation.collection != CollectionId(number as u32)
                || state.generation.name != definition.name
            {
                return Err(IndexError::IncompatibleConfiguration(format!(
                    "collection {} does not match the saved index",
                    definition.name
                )));
            }
            let source = sources
                .resolve(&definition.format)
                .map_err(IndexError::IncompatibleConfiguration)?;
            restored.push(CollectionState {
                definition: definition.clone(),
                source,
                generation: state.generation,
                postings: state.postings,
                inference: state.inference,
                global_inference: state.global_inference,
                custom_postings: state.custom_postings,
                raw: state.raw,
                fallback_text: state.fallback_text,
                logical_records: state.logical_records,
                empty_array_records: state.empty_array_records,
                custom_text: state.custom_text,
                text_bindings: crate::index_catalog::TextBindingCatalog::new(&definition.schema),
                sorting: state.sorting,
                index_family_bytes: state.index_family_bytes,
                id_lookup: state.id_lookup.into_iter().collect(),
                identity_order: state.identity_order,
                stats: state.stats,
                relations: state.relations,
            });
        }
        let repository = Arc::new(
            Repository::from_restored_parts(
                self.root,
                self.files,
                self.base_fingerprints,
                restored,
                sources,
                options,
                self.build_parses,
                self.build_failed_parses,
            )
            .map_err(IndexError::InvalidPublication)?,
        );
        let catalog = crate::lifecycle_catalog::FileCatalog::restored(
            Arc::clone(&repository),
            self.publication.catalog_slots,
        )
        .map_err(IndexError::InvalidPublication)?;
        let mut active = Vec::with_capacity(repository.collections().len());
        for (number, bitmap) in self.publication.active.into_iter().enumerate() {
            let slot_count = repository
                .collections()
                .get(number)
                .ok_or_else(|| {
                    IndexError::InvalidPublication("active collection is missing".into())
                })?
                .generation
                .records
                .len();
            active.push(
                ActiveRecords::from_bitmap(slot_count, bitmap)
                    .map_err(IndexError::InvalidPublication)?,
            );
        }
        let mut segment_groups = vec![Vec::new(); repository.collections().len()];
        for segment in self.publication.segments {
            let collection = segment.collection as usize;
            let group = segment_groups.get_mut(collection).ok_or_else(|| {
                IndexError::InvalidPublication("segment collection is out of bounds".into())
            })?;
            let slot_count = segment.index.records.len();
            if segment.index.collection != CollectionId(collection as u32) {
                return Err(IndexError::InvalidPublication(
                    "segment collection identity mismatch".into(),
                ));
            }
            group.push(Arc::new(PublishedSegment {
                ordinal: segment.ordinal,
                index: Arc::new(segment.index),
                active: ActiveRecords::from_bitmap(slot_count, segment.active)
                    .map_err(IndexError::InvalidPublication)?,
            }));
        }
        let file_versions = FileVersions::from_versions(
            self.publication
                .file_versions
                .into_iter()
                .map(FileVersion)
                .collect(),
        )
        .map_err(IndexError::InvalidPublication)?;
        if file_versions.len() != catalog.allocated_files() {
            return Err(IndexError::InvalidPublication(
                "file versions do not cover the file catalogue".into(),
            ));
        }
        let publication_id = uuid::Uuid::parse_str(&self.publication.publication_id)
            .map_err(|_| IndexError::InvalidPublication("invalid publication UUID".into()))?;
        let lineage_id = uuid::Uuid::parse_str(&self.publication.lineage_id)
            .map_err(|_| IndexError::InvalidPublication("invalid lineage UUID".into()))?;
        let published = PublishedIndex {
            instance_id: crate::repository::next_repository_instance(),
            publication_id,
            lineage_id,
            revision: IndexRevision(self.publication.revision),
            repository,
            configuration_identity,
            active,
            segments: segment_groups.into_iter().map(Arc::new).collect(),
            next_segment_ordinal: self.publication.next_segment_ordinal,
            last_size_maintenance_revision: self.publication.last_size_maintenance_revision,
            catalog,
            file_versions,
            reconciliation: ReconciliationState::NotStarted,
            processed_files: self.publication.processed_files,
            processed_memberships: Vec::new(),
        };
        Ok(Index::from_published(published))
    }
}


/// Encode one native-state object.
pub fn build_state_object(state: &NativeState) -> StorageResult<super::format::BuiltObject> {
    let bytes = codec()
        .serialize(state)
        .map_err(|error| StorageError::corrupt(format!("native state encoding: {error}")))?;
    let mut builder = ObjectBuilder::new(ObjectKind::State);
    builder.add_section(SectionKind::NativeState, bytes)?;
    builder.finish()
}

/// Decode one native-state object.
pub fn decode_state_object(backing: &Backing) -> StorageResult<NativeState> {
    let view = ObjectView::new(backing)?;
    if view.header().kind != ObjectKind::State {
        return Err(StorageError::Incompatible(
            "object is not native state".into(),
        ));
    }
    let bytes = view.section_bytes(SectionKind::NativeState)?;
    codec()
        .deserialize(bytes)
        .map_err(|error| StorageError::corrupt(format!("native state decoding: {error}")))
}
