use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Weak};

use roaring::RoaringBitmap;

use crate::index::segment::PublishedSegment;
use crate::inventory::{ActiveRecords, FileEntry};
use crate::model::{CollectionId, FileId, Value};
use crate::query::Query;
use crate::query::options::ReturnMode;
use crate::query_execution::streaming::{
    CancellationToken, ExecutionContext, ExecutionError, ResultScope, StreamControl, StreamError,
    StreamEvent, StreamOptions, StreamOutcome,
};
use crate::query_results::{SearchHit, SearchResult};
use crate::repository::Repository;

use super::{
    AvailableSearchResult, FileCatalogueRow, FileHandle, FileVersion, Index, IndexError,
    IndexFamilyEstimate, IndexInner, IndexRevision, PublishedIndex, index_family_estimate,
};

impl Index {
    /// Acquire one immutable view; no publication lock is held by its queries.
    pub fn view(&self) -> Result<IndexView, IndexError> {
        let published = self
            .0
            .published
            .read()
            .map_err(|_| IndexError::LockPoisoned)?;
        Ok(IndexView(Arc::clone(&published), Arc::downgrade(&self.0)))
    }
}

/// Pinned immutable read context for a single publication revision.
#[derive(Clone)]
pub struct IndexView(pub(crate) Arc<PublishedIndex>, Weak<IndexInner>);

/// Aggregate facts pinned to one index publication.
///
/// Logical counts come from this revision's active catalogue and liveness
/// masks. Physical counter fields may include retired entries until
/// compaction, and fields documented as build-metadata describe the completed
/// source generation from which the publication was derived.
#[derive(Clone, Debug, serde::Serialize)]
pub struct IndexObservation {
    pub files: usize,
    pub source_bytes: u64,
    pub records: usize,
    pub posting_fields: usize,
    pub sort_columns: usize,
    pub relation_count: usize,
    pub relation_edges: usize,
    pub relation_heap_bytes: usize,
    pub bigram_stores: usize,
    pub distinct_gram_keys: usize,
    pub bigram_heap_bytes: usize,
    pub inventory_heap_bytes: usize,
    pub diagnostic_heap_bytes: usize,
    pub collection_parts: Vec<CollectionIndexObservation>,
}

/// Approximate resident bytes owned by native backings and route directories
/// in this publication. Shared retained blocks are counted once in a view.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct NativeStorageEstimate {
    pub retained_body_bytes: usize,
    pub backing_column_bytes: usize,
    pub route_directory_bytes: usize,
}

/// One collection's facts for a pinned index publication. Definitions remain
/// available through [`IndexView::collection_definitions`].
///
/// `files`, `source_bytes` and `records` are logical values for this
/// publication. `posting_fields`, `sort_columns`, relation counters, bigram
/// counters are physical measurements and may include retired entries until
/// compaction. Capacity, identity and payload fields describe the completed
/// source generation retained by native construction.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CollectionIndexObservation {
    pub collection: String,
    pub files: usize,
    pub source_bytes: u64,
    pub records: usize,
    /// Capacity of the completed source generation's record metadata.
    pub record_table_capacity: usize,
    /// Retained record-ID payload bytes in the completed source metadata.
    pub unique_id_payload_bytes: usize,
    /// Capacity of the completed source generation's identity lookup.
    pub identity_lookup_capacity: usize,
    /// Identity-map entries retained by the completed source generation.
    pub shared_id_references: usize,
    pub posting_fields: usize,
    pub sort_columns: usize,
    pub relation_count: usize,
    pub relation_edges: usize,
    pub relation_heap_bytes: usize,
    /// Serialized size of the completed source generation's empty-array
    /// bitmap. Newly published segments are not included.
    pub build_empty_array_bitmap_serialized_bytes: usize,
    pub bigram_stores: usize,
    pub distinct_gram_keys: usize,
    pub bigram_heap_bytes: usize,
}

impl IndexView {
    /// File service pinned to this view, present only when explicitly installed
    /// or opened through the file-backed facade.
    pub fn file_catalog(&self) -> Option<crate::files::FileCatalogView<'_>> {
        self.repository()
            .catalog_registered
            .then_some(crate::files::FileCatalogView { view: self })
    }
    /// Startup observations belonging to this immutable publication.
    pub fn startup_stats(&self) -> &crate::repository::statistics::StartupStats {
        self.repository().startup()
    }

    /// Simultaneous high-water usage of the repository's managed streams.
    pub fn managed_stream_peak(&self) -> crate::repository::statistics::StreamMemoryStats {
        self.repository().stream_budget.peak()
    }

    pub fn build_parses(&self) -> u64 {
        self.repository().build_parses()
    }

    pub fn build_failed_parses(&self) -> u64 {
        self.repository().build_failed_parses()
    }

    pub fn segment_summaries(&self) -> Vec<crate::index::segment::NativeSegmentSummary> {
        self.repository()
            .segment_summaries_for(&self.0.segments)
            .collect()
    }

    /// Aggregate logical and physical facts pinned to this publication.
    ///
    /// Logical file counts are deduplicated globally but counted for every
    /// collection membership. Logical record counts combine the base liveness
    /// mask and published segment masks; retained source metadata rows are not
    /// counted a second time.
    pub fn index_observation(&self) -> IndexObservation {
        self.index_observation_with_segments(|_| {})
    }

    /// Fold this publication's segment summaries while optionally visiting
    /// each summary during the same traversal.
    pub fn index_observation_with_segments(
        &self,
        mut visit: impl FnMut(&crate::index::segment::NativeSegmentSummary),
    ) -> IndexObservation {
        let collections = self.repository().collections();
        let mut collection_parts = collections
            .iter()
            .map(|collection| CollectionIndexObservation {
                collection: collection.definition.name.clone(),
                files: 0,
                source_bytes: 0,
                records: self.0.active[collection.generation.collection.0 as usize].len() as usize,
                record_table_capacity: collection.generation.records.capacity(),
                unique_id_payload_bytes: collection
                    .generation
                    .records
                    .iter()
                    .map(|record| record.id.0.len() + 2 * std::mem::size_of::<usize>())
                    .sum(),
                identity_lookup_capacity: collection.id_lookup.capacity(),
                shared_id_references: collection.id_lookup.len(),
                posting_fields: 0,
                sort_columns: 0,
                relation_count: 0,
                relation_edges: 0,
                relation_heap_bytes: 0,
                build_empty_array_bitmap_serialized_bytes: collection
                    .empty_array_records
                    .serialized_size(),
                bigram_stores: 0,
                distinct_gram_keys: 0,
                bigram_heap_bytes: 0,
            })
            .collect::<Vec<_>>();

        let mut files = 0usize;
        let mut source_bytes = 0u64;
        for file in self.catalogue_rows() {
            files += 1;
            source_bytes = source_bytes.saturating_add(file.size);
            for collection in file.collections {
                if let Some(part) = collection_parts.get_mut(collection.0 as usize) {
                    part.files += 1;
                    part.source_bytes = part.source_bytes.saturating_add(file.size);
                }
            }
        }

        for segment in self.repository().segment_summaries_for(&self.0.segments) {
            visit(&segment);
            if let Some(part) = collection_parts.get_mut(segment.collection.0 as usize) {
                part.records = part.records.saturating_add(segment.active_records);
                part.posting_fields = part.posting_fields.saturating_add(segment.posting_fields);
                part.sort_columns = part.sort_columns.saturating_add(segment.sort_columns);
                part.relation_count = part.relation_count.saturating_add(segment.relation_count);
                part.relation_edges = part.relation_edges.saturating_add(segment.relation_edges);
                part.relation_heap_bytes = part
                    .relation_heap_bytes
                    .saturating_add(segment.relation_heap_bytes);
                part.bigram_stores = part.bigram_stores.saturating_add(segment.bigram_stores);
                part.distinct_gram_keys = part
                    .distinct_gram_keys
                    .saturating_add(segment.distinct_gram_keys);
                part.bigram_heap_bytes = part
                    .bigram_heap_bytes
                    .saturating_add(segment.bigram_heap_bytes);
            }
        }
        let (base_inventory_heap_bytes, diagnostic_heap_bytes) =
            self.repository().files().estimated_heap_bytes();
        let inventory_heap_bytes =
            base_inventory_heap_bytes + self.0.catalog.scalar_delta_heap_bytes();
        IndexObservation {
            files,
            source_bytes,
            records: collection_parts
                .iter()
                .map(|collection| collection.records)
                .sum(),
            posting_fields: collection_parts
                .iter()
                .map(|collection| collection.posting_fields)
                .sum(),
            sort_columns: collection_parts
                .iter()
                .map(|collection| collection.sort_columns)
                .sum(),
            relation_count: collection_parts
                .iter()
                .map(|collection| collection.relation_count)
                .sum(),
            relation_edges: collection_parts
                .iter()
                .map(|collection| collection.relation_edges)
                .sum(),
            relation_heap_bytes: collection_parts
                .iter()
                .map(|collection| collection.relation_heap_bytes)
                .sum(),
            bigram_stores: collection_parts
                .iter()
                .map(|collection| collection.bigram_stores)
                .sum(),
            distinct_gram_keys: collection_parts
                .iter()
                .map(|collection| collection.distinct_gram_keys)
                .sum(),
            bigram_heap_bytes: collection_parts
                .iter()
                .map(|collection| collection.bigram_heap_bytes)
                .sum(),
            inventory_heap_bytes,
            diagnostic_heap_bytes,
            collection_parts,
        }
    }

    /// Inventory component accounting without returning the file table.
    pub fn inventory_storage_observation(
        &self,
    ) -> (usize, usize, crate::inventory::FileStorageDiagnostic) {
        let inventory = self.repository().files();
        let (base_heap_bytes, diagnostic_bytes) = inventory.estimated_heap_bytes();
        let heap_bytes = base_heap_bytes + self.0.catalog.scalar_delta_heap_bytes();
        (heap_bytes, diagnostic_bytes, inventory.storage_diagnostic())
    }

    /// Construction status for one catalogue file in this pinned publication.
    pub fn file_status(&self, file: FileId) -> Option<crate::inventory::FileStatus> {
        self.file_entry(file).map(|entry| entry.status)
    }

    pub fn initial_construction_complete(&self) -> bool {
        self.0.catalog.construction_complete()
    }

    /// Configured collections captured by this publication.
    pub fn collection_definitions(
        &self,
    ) -> impl Iterator<Item = &crate::repository::configuration::CollectionDefinition> + '_ {
        self.repository()
            .collections()
            .iter()
            .filter_map(|state| state.file_definition.as_ref())
    }

    /// All collection declarations, including collections without a file
    /// importer, from this pinned publication.
    pub fn collection_declarations(
        &self,
    ) -> impl Iterator<Item = &crate::repository::configuration::CollectionDeclaration> + '_ {
        self.repository()
            .collections()
            .iter()
            .map(|state| &state.definition)
    }

    /// Inference observations captured by this publication, including the
    /// completed global summary of an incremental construction.
    pub fn inference_report(&self, collection: &str) -> Option<&crate::inference::InferenceReport> {
        self.repository().inference_report(collection)
    }

    pub fn inference_index_diagnostics(
        &self,
        collection: &str,
    ) -> Option<Vec<crate::inference::InferencePathDiagnostic>> {
        self.repository().inference_index_diagnostics(collection)
    }

    pub fn native_index_signature(&self) -> String {
        self.repository()
            .native_index_signature_for(&self.0.segments)
    }

    /// Visit live source metadata in file-ID order through bounded reads.
    pub fn visit_file_metadata(
        &self,
        mut visit: impl FnMut(&FileEntry) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        use crate::index::storage::file_catalog::{
            FILE_TRANSFER_LIMIT, FileCatalogSource, transfer_files,
        };
        let catalog = &self.0.catalog;
        transfer_files(
            catalog,
            catalog.high_water_mark(),
            FILE_TRANSFER_LIMIT,
            &|| false,
            |row| visit(row.entry.as_ref()),
        )
    }

    /// Number of visible file metadata entries in this revision.
    pub fn file_metadata_count(&self) -> u64 {
        self.0.catalog.active_files()
    }

    /// Borrow a row from this pinned publication without copying the corpus
    /// path catalogue or implying that the file has been parsed.
    pub fn catalogue_row(&self, id: FileId) -> Option<FileCatalogueRow<'_>> {
        if !self.0.catalog.is_active(id) {
            return None;
        }
        let file = self.0.catalog.entry(id)?;
        Some(FileCatalogueRow {
            id,
            relative_path: file.relative_path.as_path(),
            collections: &file.collections,
            size: file.revision.size,
            modified_ns: file.revision.modified_ns,
            created_ns: file.created_ns,
        })
    }

    pub fn catalogue_rows(&self) -> impl Iterator<Item = FileCatalogueRow<'_>> + '_ {
        (0..self.0.catalog.allocated_files())
            .filter_map(|number| self.catalogue_row(FileId(number as u32)))
    }

    /// Narrow index-family estimate for a direct repository, summed across the
    /// base and live lifecycle shards. See `SegmentAccounting` for the cached
    /// maintenance estimate.
    pub fn estimated_index_bytes(&self) -> usize {
        self.repository()
            .collections()
            .iter()
            .map(|collection| collection.index_family_bytes)
            .sum::<usize>()
            .saturating_add(
                self.0
                    .segments
                    .iter()
                    .flat_map(|group| group.iter())
                    .map(|segment| segment.index.accounting.index_family_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(self.estimated_construction_observation_bytes())
    }

    pub fn native_storage_estimate(&self) -> NativeStorageEstimate {
        let mut estimate = NativeStorageEstimate::default();
        let mut retained_blocks = HashSet::new();
        for segment in self.0.segments.iter().flat_map(|group| group.iter()) {
            let backing = &segment.index.backing;
            estimate.backing_column_bytes = estimate
                .backing_column_bytes
                .saturating_add(backing.estimated_column_bytes());
            if let crate::index::segment::SegmentBacking::Retained { blocks, .. } = backing {
                for block in blocks {
                    if retained_blocks.insert(block.as_ptr() as usize) {
                        estimate.retained_body_bytes =
                            estimate
                                .retained_body_bytes
                                .saturating_add(block.iter().fold(0usize, |bytes, body| {
                                    bytes
                                        .saturating_add(crate::ingestion::estimated_value_bytes(
                                            &body.value,
                                        ))
                                        .saturating_add(crate::ingestion::estimated_text_bytes(
                                            &body.text,
                                        ))
                                }));
                    }
                }
            }
        }
        estimate.route_directory_bytes = self
            .0
            .record_routes
            .iter()
            .map(|routes| routes.estimated_directory_bytes())
            .sum();
        estimate
    }

    /// Temporary provisional-build observation estimate. Completed views hold 0.
    pub fn estimated_construction_observation_bytes(&self) -> usize {
        self.0
            .segments
            .iter()
            .flat_map(|group| group.iter())
            .map(|segment| segment.construction_observation_bytes())
            .sum()
    }

    /// Per-collection base (ordinal 0) and published segment estimates. The
    /// benchmark uses this to expose shard overhead without counting it as a
    /// second resident catalogue.
    pub fn index_family_breakdown(
        &self,
    ) -> std::collections::BTreeMap<String, Vec<(u64, IndexFamilyEstimate)>> {
        self.repository()
            .collections()
            .iter()
            .enumerate()
            .map(|(number, collection)| {
                let mut parts = vec![(
                    0,
                    index_family_estimate(
                        &collection.raw,
                        &collection.fallback_text,
                        &collection.custom_text,
                        &collection.postings,
                        &collection.sorting,
                    ),
                )];
                parts.extend(self.0.segments[number].iter().map(|segment| {
                    (
                        segment.id.get(),
                        index_family_estimate(
                            &segment.index.raw,
                            &segment.index.fallback_text,
                            &segment.index.custom_text,
                            &segment.index.postings,
                            &segment.index.sorting,
                        ),
                    )
                }));
                (collection.definition.name.clone(), parts)
            })
            .collect()
    }

    pub fn estimated_inventory_bytes(&self) -> usize {
        self.repository().files().estimated_heap_bytes().0
            + self.0.catalog.scalar_delta_heap_bytes()
    }

    fn has_segments(&self) -> bool {
        self.0.segments.iter().any(|group| !group.is_empty())
    }

    fn ensure_initial_complete(&self) -> Result<(), String> {
        if self.initial_construction_complete() {
            Ok(())
        } else {
            Err(
                "initial index construction is incomplete; use the explicit available-record APIs"
                    .into(),
            )
        }
    }

    fn ensure_local_partial_query(&self, query: &Query) -> Result<(), String> {
        if self.initial_construction_complete() {
            return Ok(());
        }
        self.ensure_record_local_query(query)
    }

    fn ensure_record_local_query(&self, query: &Query) -> Result<(), String> {
        if query.options.offset != 0 {
            return Err("pagination waits for initial index construction".into());
        }
        let prepared = crate::query::prepared::prepare(self.repository(), query)?;
        if prepared.collections.iter().any(|entry| {
            entry
                .filter()
                .is_some_and(crate::query_execution::runner::bound_expr_has_relations)
                || entry
                    .fts()
                    .as_ref()
                    .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
        }) {
            return Err("relation queries wait for initial index construction".into());
        }
        Ok(())
    }

    fn stamp_result(&self, mut result: SearchResult) -> SearchResult {
        for hit in &mut result.hits {
            hit.generation = self.revision().0;
            self.remap_hit_diagnostic(hit);
        }
        result
    }

    fn remap_hit_diagnostic(&self, hit: &mut SearchHit) {
        let Some(local) = hit.diagnostic_ref.stored_index() else {
            return;
        };
        let Some(collection) = self
            .repository()
            .collections()
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
        else {
            return;
        };
        let Some(file) = hit.file.as_ref().map(|origin| origin.file_id) else {
            return;
        };
        for segment in self.0.segments[collection].iter() {
            let Some(slot) = segment.index.id_lookup.get(hit.record_id.as_str()).copied() else {
                continue;
            };
            if !segment.active.contains(slot)
                || segment.index.record(slot).is_none_or(|_| {
                    segment
                        .index
                        .backing
                        .file_locator(slot)
                        .is_none_or(|locator| locator.file != file)
                })
            {
                continue;
            }
            let Some(source) = segment.index.file(file) else {
                return;
            };
            let Some(diagnostic) = source.entry.diagnostics.get(local) else {
                return;
            };
            let Some(position) = self.0.catalog.entry(file).and_then(|entry| {
                entry
                    .diagnostics
                    .iter()
                    .position(|candidate| candidate == diagnostic)
            }) else {
                return;
            };
            let reference = crate::inventory::DiagnosticRef((position + 1) as u32);
            hit.diagnostic_ref = reference;
            if let Some(crate::query_results::Payload::LogicalUnavailable { diagnostic_ref }) =
                &mut hit.payload
            {
                *diagnostic_ref = reference;
            }
            return;
        }
    }

    pub fn instance_id(&self) -> u64 {
        self.0.instance_id
    }

    pub fn revision(&self) -> IndexRevision {
        self.0.revision
    }

    /// Fingerprint of the schema, source registrations and index settings
    /// captured by this publication.
    pub fn configuration_identity(&self) -> [u8; 32] {
        self.0.configuration_identity
    }

    pub fn file_handle(&self, file: FileId) -> Option<FileHandle> {
        self.0.catalog.is_active(file).then(|| {
            self.0.file_versions.get(file).map(|version| FileHandle {
                instance_id: self.0.instance_id,
                file_id: file,
                version,
            })
        })?
    }

    /// Resolve an active file by a repository-relative path, or by an
    /// absolute path beneath this view's source root.
    pub fn file_handle_for_path(&self, path: &Path) -> Option<FileHandle> {
        let relative = if path.is_absolute() {
            path.strip_prefix(self.0.repository.files().root()?).ok()?
        } else {
            path
        };
        self.0
            .catalog
            .id_for(relative)
            .and_then(|id| self.file_handle(id))
    }

    /// Paths of files active in this immutable publication.
    pub fn file_paths(&self) -> impl Iterator<Item = &Path> + '_ {
        (0..self.0.catalog.allocated_files()).filter_map(|number| {
            self.0
                .catalog
                .entry(FileId(number as u32))
                .map(|entry| entry.relative_path.as_path())
        })
    }

    /// Paths of active files indexed for one collection in this publication.
    pub fn file_paths_for_collection(&self, name: &str) -> impl Iterator<Item = &Path> + '_ {
        let collection_id = self
            .0
            .repository
            .collections()
            .iter()
            .find(|collection| collection.definition.name == name)
            .map(|collection| collection.generation.collection);
        (0..self.0.catalog.allocated_files()).filter_map(move |number| {
            let entry = self.0.catalog.entry(FileId(number as u32))?;
            collection_id
                .filter(|id| entry.collections.contains(id))
                .map(|_| entry.relative_path.as_path())
        })
    }

    pub fn file_version(&self, file: FileId) -> Option<FileVersion> {
        self.0
            .catalog
            .is_active(file)
            .then(|| self.0.file_versions.get(file))?
    }

    pub(crate) fn file_version_including_inactive(&self, file: FileId) -> Option<FileVersion> {
        self.0.file_versions.get(file)
    }

    pub(crate) fn file_entry(&self, file: FileId) -> Option<&FileEntry> {
        self.0.catalog.entry(file)
    }

    pub fn diagnostic(
        &self,
        file: FileId,
        reference: crate::inventory::DiagnosticRef,
    ) -> Option<&crate::inventory::SourceDiagnostic> {
        self.0
            .catalog
            .entry(file)?
            .diagnostics
            .get(reference.stored_index()?)
    }

    pub(crate) fn file_fingerprint(&self, file: FileId) -> Option<[u8; 32]> {
        self.0.catalog.fingerprint(file)
    }

    pub(crate) fn file_configuration(&self, file: FileId) -> Option<[u8; 32]> {
        self.0.catalog.configuration(file)
    }

    pub(crate) fn file_record_count(&self, file: FileId) -> Result<Option<u64>, IndexError> {
        let Some(entry) = self.file_entry(file) else {
            return Ok(None);
        };
        if !self.initial_construction_complete()
            && !entry.collections.iter().all(|collection| {
                self.0
                    .processed_memberships
                    .get(collection.0 as usize)
                    .is_some_and(|covered| covered.contains(file.0))
            })
        {
            return Ok(None);
        }
        let mut count = 0;
        for collection_id in &entry.collections {
            let number = collection_id.0 as usize;
            let collection = self.repository().collections().get(number).ok_or_else(|| {
                IndexError::InvalidPublication("file points to missing collection".into())
            })?;
            if let Some(slots) = collection.generation.file_records.records_for(file) {
                count += slots
                    .iter()
                    .filter(|slot| self.0.active[number].contains(**slot))
                    .count() as u64;
            }
            let routes = self.0.record_routes.get(number).ok_or_else(|| {
                IndexError::InvalidPublication("file points to missing collection".into())
            })?;
            let segments = self.0.segments.get(number).ok_or_else(|| {
                IndexError::InvalidPublication("file points to missing collection".into())
            })?;
            for segment_id in routes.segments_for_file(file)?.iter() {
                let segment = routes
                    .position(*segment_id)?
                    .and_then(|position| segments.get(position))
                    .ok_or_else(|| {
                        IndexError::InvalidPublication("file points to missing segment".into())
                    })?;
                if let Some(slots) = segment.index.file_records(file) {
                    count += slots
                        .iter()
                        .filter(|slot| segment.active.contains(**slot))
                        .count() as u64;
                }
            }
        }
        Ok(Some(count))
    }

    pub(crate) fn segments(&self, collection: CollectionId) -> Option<&[Arc<PublishedSegment>]> {
        self.0
            .segments
            .get(collection.0 as usize)
            .map(|segments| segments.as_slice())
    }

    pub(crate) fn repository(&self) -> &Repository {
        &self.0.repository
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        if !self.initial_construction_complete() {
            return Err("initial index construction is incomplete; use search_available".into());
        }
        self.search_published(query)
    }

    fn search_published(&self, query: &Query) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        if self.0.active.iter().any(|active| active.len() != 0) {
            return self.search_reference(query);
        }
        let owner = self.1.upgrade();
        let cache =
            owner.as_ref().map(
                |owner| crate::query_execution::runner::LifecycleCacheScope {
                    file_routes: Some(&self.0.record_routes),
                    store: &owner.verified_cache,
                    instance_id: self.instance_id(),
                },
            );
        let result = crate::query_execution::runner::search_segmented_lifecycle(
            self.repository(),
            resolved.as_ref(),
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
            cache,
        );
        result.map(|result| self.stamp_result(result))
    }

    /// Opt in to the validated records in this publication. An incomplete
    /// view cannot return a complete count or a final continuation page.
    pub fn search_available(&self, query: &Query) -> Result<AvailableSearchResult, String> {
        self.ensure_local_partial_query(query)?;
        let mut result = self.search_published(query)?;
        let complete = self.initial_construction_complete();
        if !complete {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        Ok(AvailableSearchResult {
            result,
            complete,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        })
    }

    /// Bind typed parameters before querying this pinned publication.
    pub fn search_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.search(&query.with_parameters(parameters)?)
    }

    pub fn search_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<SearchResult, ExecutionError> {
        if !self.initial_construction_complete() {
            return Err(context.error(
                crate::query_execution::streaming::ExecutionErrorKind::Execution,
                "initial index construction is incomplete; use search_available_with_context",
            ));
        }
        self.search_published_with_context(query, context)
    }

    fn search_published_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<SearchResult, ExecutionError> {
        context.enter()?;
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(
                crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
                message,
            )
        })?;
        if self.0.active.iter().any(|active| active.len() != 0) {
            return self
                .search_reference(&resolved)
                .map_err(|message| context.recorded_error(message));
        }
        let owner = self.1.upgrade();
        let cache =
            owner.as_ref().map(
                |owner| crate::query_execution::runner::LifecycleCacheScope {
                    file_routes: Some(&self.0.record_routes),
                    store: &owner.verified_cache,
                    instance_id: self.instance_id(),
                },
            );
        let result = crate::query_execution::runner::search_segmented_with_context_lifecycle(
            self.repository(),
            &resolved,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
            context,
            cache,
        );
        result.map(|result| self.stamp_result(result))
    }

    pub fn search_available_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<AvailableSearchResult, ExecutionError> {
        self.ensure_local_partial_query(query).map_err(|message| {
            context.error(
                crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
                message,
            )
        })?;
        let mut result = self.search_published_with_context(query, context)?;
        let complete = self.initial_construction_complete();
        if !complete {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        Ok(AvailableSearchResult {
            result,
            complete,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        })
    }

    /// Use a fresh request context while keeping query cancellation separate
    /// from the independent construction worker.
    pub fn search_available_with_cancellation(
        &self,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<AvailableSearchResult, ExecutionError> {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_available_with_context(query, &context)
    }

    /// Evaluate only segments published after `previous`. A caller can append
    /// these record-local matches to its previous unsorted result, including
    /// when several notifications were coalesced. A changed lineage or query
    /// with global dependencies returns `None` for a full replacement search.
    pub fn search_added_since(
        &self,
        previous: &IndexView,
        query: &Query,
    ) -> Result<Option<AvailableSearchResult>, String> {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            None,
        );
        self.search_added_since_with_context(previous, query, &context)
            .map_err(|error| error.to_string())
    }

    pub fn search_added_since_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_added_since_with_context(previous, query, &context)
    }

    /// Limit newly matching rows before selected payloads and evidence are
    /// materialized. The ordinary delta methods retain their all-match contract.
    pub fn search_added_since_bounded_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        limit: NonZeroUsize,
        cancellation: &CancellationToken,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_added_since_with_context_limit(previous, query, &context, Some(limit))
    }

    /// Check whether a full appendable page can advance its publication
    /// cursor without executing a delta. Global and changed-lineage queries
    /// still require the caller's replacement path.
    pub fn can_append_added_since_with_cancellation(
        &self,
        previous: &IndexView,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<bool, ExecutionError> {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.delta_query_if_appendable(previous, query, &context)
            .map(|query| query.is_some())
    }

    pub fn search_added_since_with_context(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        self.search_added_since_with_context_limit(previous, query, context, None)
    }

    fn delta_query_if_appendable(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<Option<Query>, ExecutionError> {
        context.enter()?;
        if self.instance_id() != previous.instance_id()
            || self.revision() <= previous.revision()
            || (self.initial_construction_complete() && !previous.initial_construction_complete())
            || !query.options.sort.is_empty()
            || query.options.offset != 0
            || query.options.count
            || self.0.segments.len() != previous.0.segments.len()
            || self
                .0
                .active
                .iter()
                .zip(&previous.0.active)
                .any(|(new, old)| new.bitmap() != old.bitmap())
        {
            return Ok(None);
        }
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(
                crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
                message,
            )
        })?;
        self.ensure_record_local_query(&resolved)
            .map_err(|message| {
                context.error(
                    crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
                    message,
                )
            })?;
        for (current, old) in self.0.segments.iter().zip(&previous.0.segments) {
            if old.len() > current.len()
                || old
                    .iter()
                    .zip(current.iter())
                    .any(|(left, right)| !Arc::ptr_eq(left, right))
            {
                return Ok(None);
            }
        }
        Ok(Some(resolved))
    }

    fn search_added_since_with_context_limit(
        &self,
        previous: &IndexView,
        query: &Query,
        context: &ExecutionContext,
        limit: Option<NonZeroUsize>,
    ) -> Result<Option<AvailableSearchResult>, ExecutionError> {
        let Some(resolved) = self.delta_query_if_appendable(previous, query, context)? else {
            return Ok(None);
        };
        let mut added = Vec::with_capacity(self.0.segments.len());
        let mut added_records = 0usize;
        for (current, old) in self.0.segments.iter().zip(&previous.0.segments) {
            let next = current[old.len()..].to_vec();
            added_records = added_records.saturating_add(
                next.iter()
                    .map(|segment| segment.active.len() as usize)
                    .sum::<usize>(),
            );
            added.push(Arc::new(next));
        }
        let mut request = resolved;
        request.options.limit = limit.map_or(added_records, |limit| added_records.min(limit.get()));
        request.options.count = false;
        let empty_base = self
            .0
            .active
            .iter()
            .map(|_| {
                ActiveRecords::from_bitmap(0, RoaringBitmap::new())
                    .expect("empty active bitmap is valid")
            })
            .collect::<Vec<_>>();
        let mut result = crate::query_execution::runner::search_segmented_with_context_lifecycle(
            self.repository(),
            &request,
            &empty_base,
            &added,
            &self.0.catalog,
            self.revision(),
            context,
            None,
        )?;
        result = self.stamp_result(result);
        result.count = None;
        result.counters.match_count_complete = false;
        Ok(Some(AvailableSearchResult {
            result,
            complete: false,
            revision: self.revision(),
            discovered_files: self.0.catalog.active_files() as usize,
            processed_files: self.0.processed_files,
        }))
    }

    /// Search only when every required source was available to this execution.
    pub fn search_strict(&self, query: &Query) -> Result<SearchResult, String> {
        let result = self.search(query)?;
        if !result.counters.match_count_complete {
            return Err("search source data is incomplete".into());
        }
        Ok(result)
    }

    /// Context-aware strict search with a typed source-change failure.
    pub fn search_strict_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<SearchResult, ExecutionError> {
        let result = self.search_with_context(query, context)?;
        if !result.counters.match_count_complete {
            return Err(context.error(
                crate::query_execution::streaming::ExecutionErrorKind::SourceChanged,
                "search source data is incomplete",
            ));
        }
        Ok(result)
    }

    pub fn count_exact(&self, query: &Query) -> Result<usize, String> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_strict(&request)?
            .count
            .ok_or("count result missing".into())
    }

    pub fn count_exact_with_context(
        &self,
        query: &Query,
        context: &ExecutionContext,
    ) -> Result<usize, ExecutionError> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_strict_with_context(&request, context)?
            .count
            .ok_or_else(|| ExecutionError {
                kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                message: "count result missing".into(),
            })
    }

    pub fn get_record(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        match self.get_record_available(collection, record_id, return_mode)? {
            crate::query_execution::runner::DirectLookupResult::Found(hit) => Ok(Some(hit)),
            crate::query_execution::runner::DirectLookupResult::NotFound => Ok(None),
            crate::query_execution::runner::DirectLookupResult::SourceUnavailable(diagnostic) => {
                Err(diagnostic.message)
            }
        }
    }

    /// Repeat a direct lookup through the production route with counters.
    /// Keep this pass separate from a timed lookup sample.
    #[doc(hidden)]
    pub fn get_record_with_route_counts_for_measurement(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
        counters: &mut crate::query_execution::runner::DirectRouteCounters,
    ) -> Result<Option<SearchHit>, String> {
        self.ensure_initial_complete()?;
        match crate::query_execution::runner::direct_lookup_segmented_counted(
            self.repository(),
            collection,
            record_id,
            return_mode,
            &self.0.active,
            &self.0.segments,
            Some(&self.0.record_routes),
            &self.0.catalog,
            self.revision(),
            Some(counters),
        )? {
            crate::query_execution::runner::DirectLookupResult::Found(mut hit) => {
                self.remap_hit_diagnostic(&mut hit);
                Ok(Some(hit))
            }
            crate::query_execution::runner::DirectLookupResult::NotFound => Ok(None),
            crate::query_execution::runner::DirectLookupResult::SourceUnavailable(diagnostic) => {
                Err(diagnostic.message)
            }
        }
    }

    /// Direct lookup with a distinct unavailable-source outcome. Explicit
    /// lifecycle operations own source reconciliation.
    pub fn get_record_available(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<crate::query_execution::runner::DirectLookupResult, String> {
        self.ensure_initial_complete()?;
        let mut result = crate::query_execution::runner::direct_lookup_segmented(
            self.repository(),
            collection,
            record_id,
            return_mode,
            &self.0.active,
            &self.0.segments,
            Some(&self.0.record_routes),
            &self.0.catalog,
            self.revision(),
        )?;
        if let crate::query_execution::runner::DirectLookupResult::Found(hit) = &mut result {
            self.remap_hit_diagnostic(hit);
        }
        Ok(result)
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.ensure_initial_complete()?;
        let resolved = query.resolve_current_clock()?;
        let query = resolved.as_ref();
        crate::query_execution::runner::search_segmented_reference(
            self.repository(),
            query,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
        )
        .map(|result| self.stamp_result(result))
    }

    pub fn search_reference_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.search_reference(&query.with_parameters(parameters)?)
    }

    pub fn select_exact(
        &self,
        collection: &str,
        query: &Query,
    ) -> Result<crate::repository::posting_values::ExactSelection, String> {
        self.ensure_initial_complete()?;
        if self.has_segments() {
            return crate::repository::posting_values::select_exact_segmented(
                self.repository(),
                collection,
                query,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository()
            .select_exact_for_view(collection, query, self.revision(), &self.0.active)
    }

    pub fn posting_values(
        &self,
        request: &crate::repository::posting_values::PostingValuesRequest,
    ) -> Result<crate::repository::posting_values::PostingValuesPage, String> {
        self.ensure_initial_complete()?;
        if self.has_segments() {
            return crate::repository::posting_values::posting_values_segmented(
                self.repository(),
                request,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository()
            .posting_values_for_view(request, self.revision(), &self.0.active)
    }

    pub fn relation_outgoing_page(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::repository::relations::OutgoingReferencePage, String> {
        self.ensure_initial_complete()?;
        if self.has_segments() {
            return crate::repository::relations::relation_outgoing_page_segmented(
                self.repository(),
                source_collection,
                relation_path,
                source_id,
                offset,
                limit,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository().relation_outgoing_page_for_view(
            source_collection,
            relation_path,
            source_id,
            offset,
            limit,
            &self.0.active,
        )
    }

    pub fn relation_backlink_page(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::repository::relations::BacklinkPage, String> {
        self.ensure_initial_complete()?;
        if self.has_segments() {
            return crate::repository::relations::relation_backlink_page_segmented(
                self.repository(),
                target_collection,
                reverse_alias,
                target_id,
                offset,
                limit,
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository().relation_backlink_page_for_view(
            target_collection,
            reverse_alias,
            target_id,
            offset,
            limit,
            &self.0.active,
        )
    }

    pub fn collection_graph(
        &self,
    ) -> Result<crate::repository::relations::CollectionGraph, String> {
        self.ensure_initial_complete()?;
        if self.has_segments() {
            return crate::repository::relations::collection_graph_segmented(
                self.repository(),
                self.revision(),
                &self.0.active,
                &self.0.segments,
                &self.0.catalog,
            );
        }
        self.repository().collection_graph_for_view(&self.0.active)
    }

    pub fn search_analyze(
        &self,
        query: &Query,
    ) -> Result<
        (
            SearchResult,
            Vec<crate::query_diagnostics::trace::QueryPlanTrace>,
        ),
        String,
    > {
        self.ensure_initial_complete()?;
        let resolved = query.resolve_current_clock()?;
        crate::query_execution::runner::search_analyze_segmented(
            self.repository(),
            resolved.as_ref(),
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
        )
        .map(|(result, trace)| (self.stamp_result(result), trace))
    }

    pub fn explain(
        &self,
        query: &Query,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        self.ensure_initial_complete()?;
        let resolved = query.resolve_current_clock()?;
        crate::query_execution::runner::explain_segmented(
            self.repository(),
            resolved.as_ref(),
            &self.0.active,
            &self.0.segments,
            self.revision(),
        )
    }

    pub fn explain_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        self.explain(&query.with_parameters(parameters)?)
    }

    pub fn explain_analyze(
        &self,
        query: &Query,
    ) -> Result<
        (
            SearchResult,
            Vec<crate::query_diagnostics::trace::QueryPlanTrace>,
        ),
        String,
    > {
        self.search_analyze(query)
    }

    pub fn search_stream<F>(
        &self,
        query: &Query,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        self.search_stream_scoped(
            query,
            ResultScope::Page {
                offset: query.options.offset,
                limit: query.options.limit,
            },
            options,
            cancellation,
            sink,
        )
    }

    pub fn search_stream_with_parameters<F>(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        let bound =
            query
                .with_parameters(parameters)
                .map_err(|message| StreamError::Execution {
                    kind: crate::query_execution::streaming::ExecutionErrorKind::InvalidQuery,
                    message,
                    progress: crate::query_execution::streaming::StreamProgress::default(),
                })?;
        self.search_stream(&bound, options, cancellation, sink)
    }

    pub fn search_stream_scoped<F>(
        &self,
        query: &Query,
        scope: ResultScope,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        let context = ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::query_execution::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_stream_scoped_with_context(query, scope, options, &context, sink)
    }

    pub fn search_stream_scoped_with_context<F>(
        &self,
        query: &Query,
        scope: ResultScope,
        options: StreamOptions,
        context: &ExecutionContext,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        if !self.initial_construction_complete() {
            return Err(StreamError::Execution {
                kind: crate::query_execution::streaming::ExecutionErrorKind::Execution,
                message: "initial index construction is incomplete; use search_available".into(),
                progress: crate::query_execution::streaming::StreamProgress::default(),
            });
        }
        let owner = self.1.upgrade();
        let cache =
            owner.as_ref().map(
                |owner| crate::query_execution::runner::LifecycleCacheScope {
                    file_routes: Some(&self.0.record_routes),
                    store: &owner.verified_cache,
                    instance_id: self.instance_id(),
                },
            );
        let mut remap_rows = |event: StreamEvent<'_>| -> Result<StreamControl, String> {
            match event {
                StreamEvent::OrderedRows {
                    sequence,
                    start,
                    rows,
                    progress,
                } => {
                    if rows.iter().any(|hit| {
                        hit.diagnostic_ref.stored_index().is_some()
                            || hit.generation != self.revision().0
                    }) {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            hit.generation = self.revision().0;
                            self.remap_hit_diagnostic(hit);
                        }
                        sink(StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows: &remapped,
                            progress,
                        })
                    } else {
                        sink(StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows,
                            progress,
                        })
                    }
                }
                StreamEvent::PageReplacement {
                    sequence,
                    rows,
                    progress,
                } => {
                    if rows.iter().any(|hit| {
                        hit.diagnostic_ref.stored_index().is_some()
                            || hit.generation != self.revision().0
                    }) {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            hit.generation = self.revision().0;
                            self.remap_hit_diagnostic(hit);
                        }
                        sink(StreamEvent::PageReplacement {
                            sequence,
                            rows: &remapped,
                            progress,
                        })
                    } else {
                        sink(StreamEvent::PageReplacement {
                            sequence,
                            rows,
                            progress,
                        })
                    }
                }
                StreamEvent::Progress { sequence, progress } => {
                    sink(StreamEvent::Progress { sequence, progress })
                }
            }
        };
        let result = crate::query_execution::runner::search_stream_segmented_with_context_lifecycle(
            self.repository(),
            query,
            scope,
            options,
            context,
            &self.0.active,
            &self.0.segments,
            &self.0.catalog,
            self.revision(),
            &mut remap_rows,
            cache,
        );
        result
    }
}
