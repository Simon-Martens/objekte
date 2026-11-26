//! Repository inventory, relation and native-state inspection.

use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::index::memory::inference::InferenceState;
use crate::index::relation::RelationIndex;
use crate::index::storage::component::ComponentKind;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::posting::{PostingKey, ScalarKey, visit_posting_blocks};
use crate::index::storage::segment::SegmentReader;
use crate::index::storage::text::visit_gram_blocks;
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inference::{InferencePathDiagnostic, InferenceReport};
use crate::inventory::FileEntry;
#[cfg(test)]
use crate::inventory::FileInventory;
use crate::model::{CollectionId, FileId, RecordSlot};

use super::Repository;
use super::configuration::RepositoryOptions;
use super::state::CollectionState;
use super::statistics::StartupStats;

fn hash_field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

impl Repository {
    #[cfg(test)]
    pub(crate) fn files(&self) -> &FileInventory {
        &self.files
    }

    pub(crate) fn base_file(&self, file: FileId) -> Result<Cow<'_, FileEntry>, String> {
        self.catalog
            .file(file)?
            .ok_or_else(|| format!("source file {} is outside the base catalog", file.0))
    }

    /// Registered source root, when the repository has a file service.
    pub fn repository_root(&self) -> Option<&std::path::Path> {
        self.source_root.as_deref()
    }

    pub(crate) fn required_file_root(&self) -> &std::path::Path {
        self.repository_root()
            .expect("file adapter requires a registered catalog root")
    }

    pub fn collections(&self) -> &[CollectionState] {
        &self.collections
    }

    /// Read one declared source relation without decoding source records.
    pub(crate) fn relation_index(
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
    pub(crate) fn backlink_relation(
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
    pub(crate) fn relation_reverse_sources(
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

    /// Compact native segment inventory for benchmarks and diagnostics.
    pub fn segment_summaries(&self) -> Vec<crate::index::segment::NativeSegmentSummary> {
        #[cfg(test)]
        if self.lookup_segment_catalogs().is_none() {
            return self
                .segments
                .iter()
                .flat_map(|group| group.iter())
                .map(|segment| segment.summary(segment.active.len() as usize))
                .collect();
        }
        let mut summaries = Vec::new();
        for directory in &self.segment_catalogs {
            directory.visit_summaries(&mut |summary| summaries.push(summary));
        }
        summaries
    }

    /// Read logical descriptors for a published segment's available components.
    pub fn segment_components(
        &self,
        collection: CollectionId,
        segment: crate::index::segment::SegmentId,
    ) -> Result<
        Option<Vec<crate::index::segment::NativeComponentDescriptor<'_>>>,
        crate::lifecycle::IndexError,
    > {
        use crate::index::storage::component::ComponentDescriptor;
        use std::borrow::Cow;
        let placement = if let Some(directories) = self.lookup_segment_catalogs() {
            let Some(directory) = directories.get(collection.0 as usize) else {
                return Ok(None);
            };
            let Some(position) = directory.position(segment)? else {
                return Ok(None);
            };
            Some(
                directory
                    .placement_at(position)?
                    .filter(|placement| placement.id == segment)
                    .ok_or_else(|| {
                        crate::index::storage::transfer::StorageError::invalid(
                            "inspect segment components",
                            "segment placement mismatch",
                        )
                    })?
                    .into_owned(),
            )
        } else {
            self.segment_groups()?
                .get(collection.0 as usize)
                .and_then(|group| group.iter().find(|placement| placement.id == segment))
                .cloned()
        };
        placement
            .map(|placement| {
                Ok(placement
                    .components()?
                    .into_iter()
                    .map(|component| ComponentDescriptor {
                        collection: component.collection,
                        segment: component.segment,
                        kind: component.kind,
                        path: component.path.map(|path| Cow::Owned(path.into_owned())),
                        binding: component
                            .binding
                            .map(|binding| Cow::Owned(binding.into_owned())),
                    })
                    .collect())
            })
            .transpose()
    }

    /// Canonical logical-content digest for the adopted native segments.
    /// Segment grouping and local slots are normalized to logical record IDs,
    /// so a policy change does not masquerade as an index-content change.
    pub fn native_index_signature(&self) -> Result<String, crate::lifecycle::IndexError> {
        self.native_index_signature_for(&self.segment_groups()?)
    }

    pub(crate) fn native_index_signature_for(
        &self,
        segments: &[Arc<Vec<Arc<crate::index::storage::publication::PublishedSegment>>>],
    ) -> Result<String, crate::lifecycle::IndexError> {
        let mut hash = Sha256::new();
        for (collection_index, collection) in self.collections.iter().enumerate() {
            hash_field(&mut hash, collection.definition.name.as_bytes());
            let records = &collection.generation.records;
            let mut components = BTreeMap::<String, Vec<String>>::new();
            for segment in segments[collection_index].iter() {
                let record_id = |slot: RecordSlot| -> StorageResult<String> {
                    let record = segment
                        .header(slot)?
                        .filter(|header| header.slot == slot)
                        .ok_or_else(|| {
                            StorageError::invalid("index signature", "missing segment header")
                        })?;
                    Ok(record.id.0.to_string())
                };
                let descriptors = segment.components()?;
                for descriptor in &descriptors {
                    let kind = descriptor.kind;
                    if !matches!(
                        kind,
                        ComponentKind::DeclaredPosting
                            | ComponentKind::InferredPosting
                            | ComponentKind::CustomPosting
                    ) {
                        continue;
                    }
                    let reader = segment.posting(descriptor)?.ok_or_else(|| {
                        StorageError::invalid("index signature", "advertised posting is missing")
                    })?;
                    let path = descriptor
                        .path
                        .as_deref()
                        .ok_or_else(|| {
                            StorageError::invalid("index signature", "missing posting path")
                        })?
                        .join(".");
                    let prefix = match kind {
                        ComponentKind::DeclaredPosting => format!("declared:{path}"),
                        ComponentKind::InferredPosting => format!("inferred:{path}"),
                        ComponentKind::CustomPosting => {
                            let binding = descriptor.binding.as_deref().ok_or_else(|| {
                                StorageError::invalid("index signature", "missing posting binding")
                            })?;
                            format!("custom:{path}:{binding}")
                        }
                        _ => unreachable!("selected a posting family"),
                    };
                    visit_posting_blocks(
                        reader,
                        segment.descriptor().slots,
                        &|| false,
                        &mut |key, slots| {
                            let label = match key {
                                PostingKey::Presence if kind != ComponentKind::CustomPosting => {
                                    format!("{prefix}:presence")
                                }
                                PostingKey::Value(ScalarKey::Null)
                                    if kind != ComponentKind::CustomPosting =>
                                {
                                    format!("{prefix}:nulls")
                                }
                                PostingKey::Value(ScalarKey::Null) => return Ok(()),
                                PostingKey::Value(key) if kind == ComponentKind::CustomPosting => {
                                    format!("{prefix}:{key:?}")
                                }
                                PostingKey::Value(key) => format!("{prefix}:value:{key:?}"),
                                PostingKey::Length(length)
                                    if kind == ComponentKind::DeclaredPosting =>
                                {
                                    format!("{prefix}:length:{length:?}")
                                }
                                _ => return Ok(()),
                            };
                            let output = components.entry(label).or_default();
                            for &slot in slots {
                                output.push(record_id(slot)?);
                            }
                            Ok(())
                        },
                    )?;
                }
                for descriptor in &descriptors {
                    let label = match descriptor.kind {
                        ComponentKind::RawText => "raw".to_owned(),
                        ComponentKind::FallbackText => "fallback".to_owned(),
                        ComponentKind::FieldText => {
                            let (Some(path), Some(binding)) =
                                (descriptor.path.as_deref(), descriptor.binding.as_deref())
                            else {
                                return Err(StorageError::invalid(
                                    "index signature",
                                    "missing field text binding",
                                )
                                .into());
                            };
                            format!("text:{}:{binding}", path.join("."))
                        }
                        _ => continue,
                    };
                    let reader = segment.grams(descriptor)?.ok_or_else(|| {
                        StorageError::invalid(
                            "index signature",
                            "advertised gram component is missing",
                        )
                    })?;
                    visit_gram_blocks(
                        reader,
                        segment.descriptor().slots,
                        &|| false,
                        &mut |channel, gram, slots| {
                            let output = components
                                .entry(format!("{label}:{}:{gram}", channel.name()))
                                .or_default();
                            for &slot in slots {
                                output.push(record_id(slot)?);
                            }
                            Ok(())
                        },
                    )?;
                }
                for column in segment.order_columns()? {
                    if column.key_count() == 0 {
                        continue;
                    }
                    let label = format!("sort:{}", column.path().display());
                    let entries = components.entry(label).or_default();
                    for slot in 0..column.key_count() {
                        let slot = RecordSlot(u32::try_from(slot).map_err(|_| {
                            crate::lifecycle::IndexError::ResourceLimit(
                                "signature sort slot out of range".into(),
                            )
                        })?);
                        let key = column.key(slot)?;
                        entries.push(format!("{}:{:?}", record_id(slot)?, key.as_ref()));
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
        Ok(format!("{:x}", hash.finalize()))
    }

    pub(crate) fn has_native_segments(&self) -> bool {
        #[cfg(test)]
        if self.segments.iter().any(|group| !group.is_empty()) {
            return true;
        }
        self.segment_catalogs
            .iter()
            .any(|directory| directory.segment_count() != 0)
    }
}
