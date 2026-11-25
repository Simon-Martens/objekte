//! Repository inventory, relation and native-state inspection.

use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::index::relation::RelationIndex;
use crate::inference::{InferencePathDiagnostic, InferenceReport, InferenceState};
use crate::inventory::FileInventory;
use crate::model::{CollectionId, RecordSlot};

use super::Repository;
use super::configuration::RepositoryOptions;
use super::state::CollectionState;
use super::statistics::StartupStats;

fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

impl Repository {
    pub fn files(&self) -> &FileInventory {
        &self.files
    }

    pub fn collections(&self) -> &[CollectionState] {
        &self.collections
    }

    /// Read one declared source relation without decoding source records.
    pub fn relation_index(
        &self,
        source_collection: &str,
        path: &[String],
    ) -> Option<&RelationIndex> {
        let collection = self
            .collections
            .iter()
            .find(|item| item.definition.name == source_collection)?;
        collection
            .relations
            .iter()
            .find(|relation| relation.declaration.path == path)
    }

    /// Resolve a reverse alias in its target collection's namespace.
    pub fn backlink_relation(
        &self,
        target_collection: &str,
        alias: &str,
    ) -> Option<&RelationIndex> {
        let target = self
            .collections
            .iter()
            .find(|item| item.definition.name == target_collection)?;
        self.collections
            .iter()
            .flat_map(|source| &source.relations)
            .find(|relation| {
                relation.target_collection == target.generation.collection
                    && relation.declaration.reverse_alias.as_deref() == Some(alias)
            })
    }

    /// Borrow the existing typed posting for sources referencing this ID.
    pub fn relation_reverse_sources(
        &self,
        relation: &RelationIndex,
        target_id: &str,
    ) -> Option<&RoaringBitmap> {
        let source = self.collections.get(relation.id.source.0 as usize)?;
        if !source
            .relations
            .iter()
            .any(|registered| std::ptr::eq(registered, relation))
        {
            return None;
        }
        source
            .postings
            .fields
            .get(&relation.declaration.path)?
            .values
            .get(&crate::index::posting::ScalarKey::Id(target_id.to_owned()))
    }

    pub fn build_parses(&self) -> u64 {
        self.build_parses
    }

    pub fn build_failed_parses(&self) -> u64 {
        self.build_failed_parses
    }

    /// Opening configuration. The verified cache budget may subsequently be
    /// changed through `configure_verified_cache`.
    pub fn options(&self) -> RepositoryOptions {
        self.options
    }

    /// Completed construction-generation observations for a collection.
    ///
    /// File mutations keep this report tied to the construction generation
    /// whose segments produced it; compaction or rebuild paths mark it
    /// incomplete until a new collection decision is available.
    pub fn inference_report(&self, collection: &str) -> Option<&InferenceReport> {
        self.collections
            .iter()
            .find(|state| state.definition.name == collection)
            .and_then(|state| {
                state
                    .global_inference
                    .as_ref()
                    .map(|summary| &summary.report)
                    .or_else(|| state.inference.as_ref().map(|inference| &inference.report))
            })
    }

    /// Ordered-posting outcomes for every observed path, sorted by typed path.
    /// Returns `None` when inference was disabled for the collection.
    pub fn inference_index_diagnostics(
        &self,
        collection: &str,
    ) -> Option<Vec<InferencePathDiagnostic>> {
        self.collections
            .iter()
            .find(|state| state.definition.name == collection)
            .and_then(|state| {
                state
                    .global_inference
                    .as_ref()
                    .map(|summary| summary.diagnostics.clone())
                    .or_else(|| {
                        state
                            .inference
                            .as_ref()
                            .map(InferenceState::path_diagnostics)
                    })
            })
    }

    pub fn verified_cache_budget(&self) -> Result<usize, String> {
        Ok(self.verified.lock().map_err(|e| e.to_string())?.budget())
    }

    pub fn startup(&self) -> &StartupStats {
        &self.startup
    }

    pub(crate) fn native_snapshot(&self) -> crate::index::segment::NativeSnapshotRef<'_> {
        crate::index::segment::NativeSnapshotRef {
            active: &self.active,
            segments: &self.segments,
            catalog: &self.catalog,
            revision: self.revision,
        }
    }

    #[allow(
        dead_code,
        reason = "retained as the collection-scoped segment read-view adapter"
    )]
    pub(crate) fn segment_read_views(
        &self,
        collection: CollectionId,
    ) -> impl Iterator<Item = crate::index::segment::SegmentReadView<'_>> {
        self.segments
            .get(collection.0 as usize)
            .into_iter()
            .flat_map(|segments| segments.iter())
            .map(|segment| crate::index::segment::IndexedCollectionSegment::read_view(segment))
    }

    /// Compact native segment inventory for benchmarks and diagnostics.
    pub fn segment_summaries(&self) -> Vec<crate::index::segment::NativeSegmentSummary> {
        self.segment_summaries_for(&self.segments).collect()
    }

    pub(crate) fn segment_summaries_for<'a>(
        &self,
        segments: &'a [Arc<Vec<Arc<crate::index::segment::PublishedSegment>>>],
    ) -> impl Iterator<Item = crate::index::segment::NativeSegmentSummary> + 'a {
        segments
            .iter()
            .flat_map(|segments| {
                segments.iter().map(|segment| {
                    crate::index::segment::IndexedCollectionSegment::read_view(segment)
                })
            })
            .map(|view| crate::index::segment::NativeSegmentSummary {
                id: view.id,
                collection: view.collection,
                records: view.index.record_count(),
                active_records: view.active.len() as usize,
                files: view.index.file_count(),
                source_bytes: view.index.accounting.source_bytes_estimated,
                posting_fields: view.index.postings.fields.len(),
                sort_columns: view.index.sorting.columns.len(),
                relation_count: view.index.relations.len(),
                relation_edges: view
                    .index
                    .relations
                    .iter()
                    .map(|relation| relation.edge_count())
                    .sum(),
                relation_heap_bytes: view
                    .index
                    .relations
                    .iter()
                    .map(|relation| relation.estimated_bytes())
                    .sum(),
                bigram_stores: 2 + view.index.custom_text.len(),
                distinct_gram_keys: std::iter::once(&*view.index.raw)
                    .chain(std::iter::once(&*view.index.fallback_text))
                    .chain(view.index.custom_text.values())
                    .map(|index| index.normal.len() + index.skip.len())
                    .sum(),
                bigram_heap_bytes: std::iter::once(&*view.index.raw)
                    .chain(std::iter::once(&*view.index.fallback_text))
                    .chain(view.index.custom_text.values())
                    .map(crate::index::bigram::BigramIndex::estimated_heap_bytes)
                    .sum(),
                index_family_bytes: view.index.accounting.index_family_bytes,
                active: view.active.len() != 0,
            })
    }

    /// Borrowed descriptors for one segment's independently owned native
    /// components.
    pub fn segment_components(
        &self,
        collection: CollectionId,
        segment: crate::index::segment::SegmentId,
    ) -> Option<Vec<crate::index::segment::NativeComponentDescriptor<'_>>> {
        self.segments
            .get(collection.0 as usize)?
            .iter()
            .find(|candidate| candidate.id == segment)
            .map(|candidate| candidate.index.component_descriptors(candidate.id))
    }

    /// Canonical logical-content digest for the adopted native segments.
    /// Segment grouping and local slots are normalized to logical record IDs,
    /// so a policy change does not masquerade as an index-content change.
    pub fn native_index_signature(&self) -> String {
        self.native_index_signature_for(&self.segments)
    }

    pub(crate) fn native_index_signature_for(
        &self,
        segments: &[Arc<Vec<Arc<crate::index::segment::PublishedSegment>>>],
    ) -> String {
        let mut hash = Sha256::new();
        for (collection_index, collection) in self.collections.iter().enumerate() {
            hash_field(&mut hash, collection.definition.name.as_bytes());
            let records = &collection.generation.records;
            let mut components = BTreeMap::<String, Vec<String>>::new();
            for segment in segments[collection_index].iter() {
                let record_id = |slot: RecordSlot| {
                    segment
                        .index
                        .record(slot)
                        .map(|record| record.id.0.to_string())
                        .unwrap_or_else(|| format!("missing:{}", slot.0))
                };
                let add_bitmap = |components: &mut BTreeMap<String, Vec<String>>,
                                  label: String,
                                  bitmap: &RoaringBitmap| {
                    components
                        .entry(label)
                        .or_default()
                        .extend(bitmap.iter().map(|slot| record_id(RecordSlot(slot))));
                };
                for (path, posting) in &segment.index.postings.fields {
                    let path = path.join(".");
                    add_bitmap(
                        &mut components,
                        format!("declared:{path}:presence"),
                        &posting.presence,
                    );
                    if let Some(nulls) = posting.null_posting() {
                        add_bitmap(&mut components, format!("declared:{path}:nulls"), nulls);
                    }
                    for (key, bitmap) in &posting.values {
                        if matches!(key, crate::index::posting::ScalarKey::Null) {
                            continue;
                        }
                        add_bitmap(
                            &mut components,
                            format!("declared:{path}:value:{key:?}"),
                            bitmap,
                        );
                    }
                    for (key, bitmap) in &posting.lengths {
                        add_bitmap(
                            &mut components,
                            format!("declared:{path}:length:{key:?}"),
                            bitmap,
                        );
                    }
                }
                if let Some(inference) = &segment.index.inference {
                    for (path, posting) in &inference.indexes {
                        let path = path.join(".");
                        add_bitmap(
                            &mut components,
                            format!("inferred:{path}:presence"),
                            &posting.presence,
                        );
                        if let Some(nulls) = posting.null_posting() {
                            add_bitmap(&mut components, format!("inferred:{path}:nulls"), nulls);
                        }
                        for (key, bitmap) in &posting.values {
                            if matches!(key, crate::index::posting::ScalarKey::Null) {
                                continue;
                            }
                            add_bitmap(
                                &mut components,
                                format!("inferred:{path}:value:{key:?}"),
                                bitmap,
                            );
                        }
                    }
                }
                for ((path, binding), posting) in &segment.index.custom_postings {
                    for (key, bitmap) in &posting.index.values {
                        if matches!(key, crate::index::posting::ScalarKey::Null) {
                            continue;
                        }
                        add_bitmap(
                            &mut components,
                            format!("custom:{}:{binding}:{key:?}", path.join(".")),
                            bitmap,
                        );
                    }
                }
                for (label, index) in [
                    ("raw".to_owned(), &segment.index.raw),
                    ("fallback".to_owned(), &segment.index.fallback_text),
                ] {
                    for (kind, map) in [("normal", &index.normal), ("skip", &index.skip)] {
                        for (key, bitmap) in map {
                            add_bitmap(&mut components, format!("{label}:{kind}:{key}"), bitmap);
                        }
                    }
                }
                for ((path, binding), index) in &segment.index.custom_text {
                    for (kind, map) in [("normal", &index.normal), ("skip", &index.skip)] {
                        for (key, bitmap) in map {
                            add_bitmap(
                                &mut components,
                                format!("text:{}:{binding}:{kind}:{key}", path.join(".")),
                                bitmap,
                            );
                        }
                    }
                }
                for (path, column) in &segment.index.sorting.columns {
                    for (slot, key) in column.keys.iter().enumerate() {
                        let slot = RecordSlot(u32::try_from(slot).unwrap_or(u32::MAX));
                        components
                            .entry(format!("sort:{}", path.display()))
                            .or_default()
                            .push(format!("{}:{key:?}", record_id(slot)));
                    }
                }
            }
            for (label, ids) in &mut components {
                ids.sort();
                ids.dedup();
                hash_field(&mut hash, label.as_bytes());
                for id in ids {
                    hash_field(&mut hash, id.as_bytes());
                }
            }
            for relation in &collection.relations {
                hash_field(&mut hash, relation.declaration.path.join(".").as_bytes());
                for record in records {
                    if let Some(edges) = relation.edges(record.slot) {
                        for edge in edges {
                            hash_field(&mut hash, record.id.0.as_bytes());
                            hash_field(&mut hash, edge.target_id().as_bytes());
                            for position in &edge.positions {
                                hash_field(&mut hash, &position.to_le_bytes());
                            }
                        }
                    }
                }
            }
        }
        format!("{:x}", hash.finalize())
    }

    pub(super) fn has_native_segments(&self) -> bool {
        self.segments.iter().any(|segments| !segments.is_empty())
    }
}
