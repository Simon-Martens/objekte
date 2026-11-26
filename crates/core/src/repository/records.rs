//! Repository record lookup and hit stamping.

use crate::index::storage::segment::SegmentReader;
use crate::inventory::{ActiveRecords, RecordClass};
use crate::lifecycle::IndexRevision;
use crate::model::RecordId;
use crate::query::options::ReturnMode;
use crate::query_results::{Payload, SearchHit, SearchResult};
use crate::source::access::SourceAccessSession;

use super::Repository;

impl Repository {
    pub fn get_record(
        &self,
        collection_name: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        self.get_record_with_active(collection_name, record_id, return_mode, None)
    }

    pub(crate) fn get_record_with_active(
        &self,
        collection_name: &str,
        record_id: &str,
        return_mode: ReturnMode,
        active: Option<&[ActiveRecords]>,
    ) -> Result<Option<SearchHit>, String> {
        if self.has_native_segments() {
            let domains = active.unwrap_or(&self.active);
            return match crate::query_execution::runner::direct_lookup_segmented(
                self,
                collection_name,
                record_id,
                return_mode,
                domains,
                &self.segments,
                None,
                &self.catalog,
                self.revision,
            )? {
                crate::query_execution::runner::DirectLookupResult::Found(mut hit) => {
                    self.stamp_native_hit(&mut hit)?;
                    Ok(Some(hit))
                }
                crate::query_execution::runner::DirectLookupResult::NotFound => Ok(None),
                crate::query_execution::runner::DirectLookupResult::SourceUnavailable(
                    diagnostic,
                ) => Err(diagnostic.message),
            };
        }
        let collection_index = self
            .collections
            .iter()
            .position(|collection| collection.definition.name == collection_name)
            .ok_or("unknown collection")?;
        let collection = &self.collections[collection_index];
        let Some(slot) = collection.id_lookup.get(&RecordId(record_id.into())) else {
            return Ok(None);
        };
        if active.is_some_and(|domains| !domains[collection_index].contains(*slot)) {
            return Ok(None);
        }
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("record ID points outside generation")?;
        let payload = match return_mode {
            ReturnMode::Locator => None,
            ReturnMode::Logical if record.class == RecordClass::RawFallback => {
                Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                })
            }
            ReturnMode::Bytes | ReturnMode::Logical => {
                let mut access = SourceAccessSession::new(&self.catalog);
                access.select_backend(&collection.required_file_reader());
                let mut read = access.read(record.file, &[(slot.0, record.range)])?;
                let bytes = read.records.pop().ok_or("source record missing")?.1;
                crate::query_results::payload::payload_from_bytes(
                    record.class,
                    Some(collection),
                    &return_mode,
                    bytes,
                )?
                .0
            }
        };
        crate::query_execution::runner::make_hit(
            self,
            collection_index,
            record,
            Vec::new(),
            payload,
        )
        .map(Some)
    }

    pub(super) fn stamp_native_result(
        &self,
        mut result: SearchResult,
    ) -> Result<SearchResult, String> {
        for hit in &mut result.hits {
            self.stamp_native_hit(hit)?;
        }
        Ok(result)
    }

    pub(super) fn stamp_native_query_result(
        &self,
        query: &crate::query::Query,
        mut result: SearchResult,
    ) -> Result<SearchResult, String> {
        if query.options.return_mode != crate::query::options::ReturnMode::Locator
            && result.counters.source_unavailable_events > 0
        {
            result.count = None;
            result.counters.match_count_complete = false;
        }
        self.stamp_native_result(result)
    }

    pub(crate) fn stamp_native_hit(&self, hit: &mut SearchHit) -> Result<(), String> {
        if let Some(collection) = self
            .collections
            .iter()
            .find(|collection| collection.definition.name == hit.collection)
        {
            hit.generation = collection.generation.generation;
        }
        self.remap_native_diagnostic(hit)
    }

    pub(crate) fn stamp_native_hit_at(
        &self,
        hit: &mut SearchHit,
        revision: IndexRevision,
    ) -> Result<(), String> {
        hit.generation = revision.0;
        self.remap_native_diagnostic(hit)
    }

    fn remap_native_diagnostic(&self, hit: &mut SearchHit) -> Result<(), String> {
        if hit.diagnostic_ref.stored_index().is_none() || hit.file.is_none() {
            return Ok(());
        }
        let Some(collection) = self
            .collections
            .iter()
            .position(|collection| collection.definition.name == hit.collection)
        else {
            return Ok(());
        };
        for segment in self.segments[collection].iter() {
            if let Some(slot) = segment.index.slot_for_record(&hit.record_id)?
                && segment.active.contains(slot)
            {
                hit.remap_segment_diagnostic(segment.index.as_ref(), &self.catalog, slot)?;
                break;
            }
        }
        Ok(())
    }
}
