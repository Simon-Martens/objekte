use std::collections::HashMap;

use roaring::RoaringBitmap;

use super::{IndexedCollectionSegment, SegmentAccounting, SegmentFile};
use crate::index::memory::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::memory::inventory::append_shifted_bitmap;
use crate::index::memory::sorting::compare_composite;
use crate::inference::{InferenceIndexStatus, InferredFieldReport, OccurrenceCountStatus};
#[cfg(test)]
use crate::inventory::ActiveRecords;
use crate::inventory::RecordClass;
use crate::model::RecordSlot;

/// The offsets translate old shard-local slots to the consolidated local
/// namespace. Publication must translate each old active mask with them.
pub(crate) struct ConsolidatedSegment {
    pub index: IndexedCollectionSegment,
    pub offsets: Vec<u32>,
}

#[cfg(test)]
impl ConsolidatedSegment {
    pub(crate) fn translate_active(
        &self,
        factory: &dyn crate::index::storage::StorageFactory,
        masks: &[&ActiveRecords],
    ) -> crate::index::storage::transfer::StorageResult<ActiveRecords> {
        crate::index::storage::segment::maintenance::translate_active(
            self.index.records.len(),
            &self.offsets,
            factory,
            masks,
        )
    }
}

fn merge_bigram_index(
    target: &mut BigramIndex,
    source: &BigramIndex,
    base: u32,
) -> Result<(), String> {
    if target.alphabet != source.alphabet || target.record_count != base {
        return Err("bigram segment representation or coverage mismatch".into());
    }
    target.record_count = base
        .checked_add(source.record_count)
        .ok_or("bigram slot overflow")?;
    source.for_each_normal(|key, posting| {
        append_shifted_bitmap(target.normal.entry(*key).or_default(), posting, base)
    })?;
    source.for_each_skip(|key, posting| {
        append_shifted_bitmap(target.skip.entry(*key).or_default(), posting, base)
    })?;
    Ok(())
}

fn consolidate_inference(
    shards: &[&IndexedCollectionSegment],
    logical: &RoaringBitmap,
    record_count: usize,
    offsets: &[u32],
) -> Result<Option<InferenceState>, String> {
    let states = shards
        .iter()
        .map(|shard| shard.inference.as_ref())
        .collect::<Option<Vec<_>>>();
    let Some(states) = states else {
        if shards.iter().any(|shard| shard.inference.is_some()) {
            return Err("inference segment mode mismatch".into());
        }
        return Ok(None);
    };
    let first = *states.first().ok_or("no inference segments")?;
    let mode = first.report.mode;
    let policy = first.report.policy;
    if states
        .iter()
        .any(|state| state.report.mode != mode || state.report.policy != policy)
    {
        return Err("inference segment configuration mismatch".into());
    }
    let complete = states.iter().all(|state| state.report.complete);
    let mut fields: HashMap<Vec<String>, InferredFieldReport> = HashMap::new();
    for state in &states {
        for source in &state.report.fields {
            let target = fields.entry(source.path.clone()).or_insert_with(|| {
                let mut field = source.clone();
                field.scalar_records = 0;
                field.array_records = 0;
                field.object_records = 0;
                field.eligible_records = 0;
                field.unavailable_records = 0;
                field.present_records = 0;
                field.missing_records = 0;
                field.null_occurrence_records = 0;
                field.exact_null_records = 0;
                field.empty_array_records = 0;
                field.empty_selection_records = 0;
                field.value_occurrences = 0;
                field.nonblank_occurrences = 0;
                field.nonblank_records = 0;
                field.short_string_observed_occurrences = 0;
                field.short_string_covered_records = 0;
                field.short_string_unresolved_records = 0;
                field.summary_bytes_estimate = 0;
                field.index_bytes_estimate = 0;
                field.retained_bytes_estimate = 0;
                field
            });
            for kind in &source.observed_kinds {
                if !target.observed_kinds.contains(kind) {
                    target.observed_kinds.push(kind.clone());
                }
            }
            target.scalar_records = target.scalar_records.saturating_add(source.scalar_records);
            target.array_records = target.array_records.saturating_add(source.array_records);
            target.object_records = target.object_records.saturating_add(source.object_records);
            target.array_nesting = target.array_nesting.max(source.array_nesting);
            target.integer_only = match (target.integer_only, source.integer_only) {
                (Some(left), Some(right)) => Some(left && right),
                _ => None,
            };
            target.present_records = target
                .present_records
                .saturating_add(source.present_records);
            target.null_occurrence_records = target
                .null_occurrence_records
                .saturating_add(source.null_occurrence_records);
            target.exact_null_records = target
                .exact_null_records
                .saturating_add(source.exact_null_records);
            target.empty_array_records = target
                .empty_array_records
                .saturating_add(source.empty_array_records);
            target.empty_selection_records = target
                .empty_selection_records
                .saturating_add(source.empty_selection_records);
            target.value_occurrences = target
                .value_occurrences
                .checked_add(source.value_occurrences)
                .ok_or("inference value occurrence overflow")?;
            target.nonblank_occurrences = target
                .nonblank_occurrences
                .checked_add(source.nonblank_occurrences)
                .ok_or("inference nonblank occurrence overflow")?;
            target.nonblank_records = target
                .nonblank_records
                .saturating_add(source.nonblank_records);
            target.short_string_observed_occurrences = target
                .short_string_observed_occurrences
                .checked_add(source.short_string_observed_occurrences)
                .ok_or("inference short-string occurrence overflow")?;
            target.occurrence_count_status = match (
                target.occurrence_count_status,
                source.occurrence_count_status,
            ) {
                (OccurrenceCountStatus::IncompleteObservation, _)
                | (_, OccurrenceCountStatus::IncompleteObservation) => {
                    OccurrenceCountStatus::IncompleteObservation
                }
                (OccurrenceCountStatus::UpperBound, _) | (_, OccurrenceCountStatus::UpperBound) => {
                    OccurrenceCountStatus::UpperBound
                }
                _ => OccurrenceCountStatus::Exact,
            };
            target.complete &= source.complete;
            target.summary_bytes_estimate = target
                .summary_bytes_estimate
                .saturating_add(source.summary_bytes_estimate);
            target.index_bytes_estimate = target
                .index_bytes_estimate
                .saturating_add(source.index_bytes_estimate);
        }
    }
    for field in fields.values_mut() {
        let missing_from_interrupted = states.iter().zip(shards).any(|(state, shard)| {
            !shard.logical_records.is_empty()
                && !state.report.complete
                && !state
                    .report
                    .fields
                    .iter()
                    .any(|source| source.path == field.path)
        });
        if missing_from_interrupted {
            field.occurrence_count_status = OccurrenceCountStatus::IncompleteObservation;
        }
    }
    let mut indexes = HashMap::new();
    let mut short_string_coverage = HashMap::new();
    if complete {
        let seed = shards
            .iter()
            .position(|shard| !shard.logical_records.is_empty());
        for (path, initial) in seed
            .into_iter()
            .flat_map(|position| &states[position].indexes)
        {
            if states.iter().zip(shards).any(|(state, shard)| {
                !shard.logical_records.is_empty()
                    && state.indexes.get(path).is_none_or(|index| {
                        index.kind != initial.kind || index.multi != initial.multi
                    })
            }) {
                continue;
            }
            let mut index = initial.clone();
            index.presence.clear();
            index.values.clear();
            index.lengths.clear();
            index.covered_records = 0;
            let mut short = None;
            for ((state, shard), base) in states.iter().zip(shards).zip(offsets) {
                if shard.logical_records.is_empty() {
                    continue;
                }
                index.merge_shifted(state.indexes[path].clone(), *base)?;
                if let Some(source) = state.short_string_coverage.get(path) {
                    let target = short.get_or_insert_with(RoaringBitmap::new);
                    append_shifted_bitmap(target, source, *base)?;
                }
            }
            index.finish_stats();
            index.validate_eligible(record_count, logical.len() as usize)?;
            index.validate_domain(logical)?;
            if let Some(short) = short {
                short_string_coverage.insert(path.clone(), short);
            }
            indexes.insert(path.clone(), index);
        }
    }
    let eligible = logical.len();
    let unavailable = record_count as u64 - eligible;
    let mut fields = fields.into_values().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.path.cmp(&right.path));
    for field in &mut fields {
        if field.complete
            && let Some(index) = indexes.get(&field.path)
            && index.kind == crate::model::LogicalType::Number
            && index.value_complete
            && index.summary.is_some_and(|summary| {
                !summary.may_match_array()
                    && !summary.may_select_multiple()
                    && !summary.may_have_empty_traversal()
            })
        {
            let nulls = index
                .values
                .get(&crate::index::memory::posting::ScalarKey::Null)
                .map_or(0, RoaringBitmap::len);
            field.integer_only = if index.values.keys().any(|key| {
                matches!(
                    key,
                    crate::index::memory::posting::ScalarKey::Number(crate::model::Number::Float(
                        _
                    ))
                )
            }) {
                Some(false)
            } else if index.presence.len() > nulls {
                Some(true)
            } else {
                None
            };
        }
        field.eligible_records = eligible;
        field.unavailable_records = unavailable;
        field.missing_records = eligible.saturating_sub(field.present_records);
        field.distinct_nonblank_strings = None;
        if let Some(index) = indexes.get(&field.path) {
            field.index_status = InferenceIndexStatus::Built;
            field.complete &= index.summary.is_some();
            if !field.observed_kinds.contains(&index.kind) {
                field.observed_kinds.push(index.kind.clone());
            }
            field.short_string_covered_records = short_string_coverage
                .get(&field.path)
                .map_or(0, RoaringBitmap::len);
            field.short_string_distinct_values = if field.short_string_covered_records > 0 {
                index.distinct_values()
            } else {
                0
            };
        } else {
            field.index_status = InferenceIndexStatus::Incomplete;
            field.complete = false;
            field.reason = "consolidated without a complete inferred posting".into();
            field.index_bytes_estimate = 0;
            field.short_string_covered_records = 0;
            field.short_string_distinct_values = 0;
        }
        field.short_string_unresolved_records = eligible - field.short_string_covered_records;
        field.retained_bytes_estimate = field
            .summary_bytes_estimate
            .saturating_add(field.index_bytes_estimate);
    }
    let mut report = first.report.clone();
    report.eligible_records = eligible;
    report.unavailable_records = unavailable;
    report.omitted_paths = states.iter().fold(0u64, |total, state| {
        total.saturating_add(state.report.omitted_paths)
    });
    report.complete = complete;
    report.observed_paths = fields.len();
    report.admitted_paths = indexes.len();
    report.dropped_paths = fields
        .iter()
        .filter(|field| {
            matches!(
                field.index_status,
                InferenceIndexStatus::Incompatible
                    | InferenceIndexStatus::Incomplete
                    | InferenceIndexStatus::BudgetExceeded
                    | InferenceIndexStatus::BelowThreshold
            )
        })
        .count();
    report.summary_bytes_estimate = fields.iter().fold(0usize, |total, field| {
        total.saturating_add(field.summary_bytes_estimate)
    });
    report.index_bytes_estimate = fields.iter().fold(0usize, |total, field| {
        total.saturating_add(field.index_bytes_estimate)
    });
    report.retained_bytes_estimate = report
        .summary_bytes_estimate
        .saturating_add(report.index_bytes_estimate);
    report.fields = fields;
    Ok(Some(InferenceState {
        report,
        indexes,
        short_string_coverage,
    }))
}

impl IndexedCollectionSegment {
    /// Combine already indexed shards without opening or parsing source files.
    /// Shards with overlapping FileIds or logical IDs must first be resolved
    /// by the coordinator's active-version policy. `max_records` bounds this
    /// consolidation's temporary copying and future shard scan cost.
    pub fn consolidate(
        shards: &[&Self],
        max_records: usize,
    ) -> Result<ConsolidatedSegment, String> {
        let first = *shards.first().ok_or("no segments to consolidate")?;
        let total_records = shards.iter().try_fold(0usize, |total, shard| {
            total
                .checked_add(shard.record_count())
                .ok_or("segment record count overflow")
        })?;
        if total_records > max_records || total_records > u32::MAX as usize {
            return Err("consolidation record budget exceeded".into());
        }
        let mut offsets = Vec::with_capacity(shards.len());
        let mut files = Vec::new();
        let mut file_index = HashMap::new();
        let mut records = Vec::with_capacity(total_records);
        let mut id_lookup = HashMap::new();
        let mut postings = (*first.postings).clone();
        let mut custom_postings = first.custom_postings.clone();
        let mut raw = (*first.raw).clone();
        let mut fallback_text = (*first.fallback_text).clone();
        let mut custom_text = first.custom_text.clone();
        let mut logical_records = RoaringBitmap::new();
        let mut empty_array_records = RoaringBitmap::new();
        let (mut sorting, mut placed_orders) = first.sorting.consolidation_columns()?;
        let mut relations = first.relations.clone();
        let mut record_parses = 0u64;
        let mut failed_parses = 0u64;
        for (number, shard) in shards.iter().enumerate() {
            if shard.collection != first.collection
                || shard.source_id != first.source_id
                || shard.source_version != first.source_version
            {
                return Err("segment collection or source binding mismatch".into());
            }
            let base = u32::try_from(records.len()).map_err(|_| "segment slot overflow")?;
            offsets.push(base);
            for file_number in 0..shard.file_count() {
                let file = shard
                    .files
                    .get(file_number)
                    .ok_or("segment file row missing")?;
                let mut entry = file.entry.clone();
                if file_index.insert(entry.id, files.len()).is_some() {
                    return Err("overlapping physical file in consolidation".into());
                }
                for diagnostic in &mut entry.diagnostics {
                    if let Some(slot) = diagnostic.record {
                        diagnostic.record = Some(RecordSlot(
                            base.checked_add(slot.0).ok_or("diagnostic slot overflow")?,
                        ));
                    }
                }
                let mut owned = Vec::with_capacity(file.records.len());
                for slot in &file.records {
                    owned.push(RecordSlot(
                        base.checked_add(slot.0).ok_or("file slot overflow")?,
                    ));
                }
                files.push(SegmentFile {
                    entry,
                    records: owned,
                });
            }
            for slot in 0..shard.record_count() {
                let mut record = shard
                    .record(RecordSlot(slot as u32))
                    .ok_or("segment record row missing")?
                    .clone();
                record.slot = RecordSlot(
                    base.checked_add(record.slot.0)
                        .ok_or("record slot overflow")?,
                );
                if id_lookup.insert(record.id.clone(), record.slot).is_some() {
                    return Err("overlapping record ID in consolidation".into());
                }
                records.push(record);
            }
            append_shifted_bitmap(&mut logical_records, &shard.logical_records, base)?;
            append_shifted_bitmap(&mut empty_array_records, &shard.empty_array_records, base)?;
            if number != 0 {
                postings.merge_shifted((*shard.postings).clone(), base)?;
                if custom_postings.len() != shard.custom_postings.len() {
                    return Err("custom posting segment coverage mismatch".into());
                }
                for (key, source) in &shard.custom_postings {
                    let target = custom_postings
                        .get_mut(key)
                        .ok_or("custom posting segment missing")?;
                    target.index.merge_shifted(source.index.clone(), base)?;
                    target.complete &= source.complete;
                }
                merge_bigram_index(&mut raw, &shard.raw, base)?;
                merge_bigram_index(&mut fallback_text, &shard.fallback_text, base)?;
                if custom_text.len() != shard.custom_text.len() {
                    return Err("custom bigram segment coverage mismatch".into());
                }
                for (key, source) in &shard.custom_text {
                    merge_bigram_index(
                        custom_text
                            .get_mut(key)
                            .ok_or("custom bigram segment missing")?,
                        source,
                        base,
                    )?;
                }
                if relations.len() != shard.relations.len() {
                    return Err("relation segment coverage mismatch".into());
                }
                for (target, source) in relations.iter_mut().zip(&shard.relations) {
                    target.merge_reader(source, base, shard.record_count() as u32)?;
                }
                if sorting.column_count() != shard.sorting.column_count()
                    || sorting.composites.len() != shard.sorting.composites.len()
                {
                    return Err("sort segment coverage mismatch".into());
                }
                sorting.append_consolidation_columns(&shard.sorting, &mut placed_orders)?;
                for (target, source) in sorting.composites.iter().zip(&shard.sorting.composites) {
                    if target.fields != source.fields {
                        return Err("composite sort representation mismatch".into());
                    }
                }
            }
            record_parses = record_parses.saturating_add(shard.record_parses);
            failed_parses = failed_parses.saturating_add(shard.failed_parses);
        }
        postings.finish_stats();
        let logical_count = logical_records.len() as usize;
        postings.validate_eligible(records.len(), logical_count)?;
        if logical_count != records.len() {
            postings.validate_domain(&logical_records)?;
        }
        for custom in custom_postings.values_mut() {
            custom.index.finish_stats();
            custom
                .index
                .validate_eligible(records.len(), logical_count)?;
            if logical_count != records.len() {
                custom.index.validate_domain(&logical_records)?;
            }
        }
        raw.validate(records.len())?;
        fallback_text.validate(records.len())?;
        if logical_count != records.len() {
            raw.validate_domain(&logical_records)?;
        }
        let fallback_slots: RoaringBitmap = records
            .iter()
            .filter(|record| record.class == RecordClass::RawFallback)
            .map(|record| record.slot.0)
            .collect();
        fallback_text.validate_domain(&fallback_slots)?;
        for index in custom_text.values() {
            index.validate(records.len())?;
            if logical_count != records.len() {
                index.validate_domain(&logical_records)?;
            }
        }
        for column in sorting.columns.values_mut() {
            if column.keys.len() != records.len() {
                return Err("sort column coverage mismatch".into());
            }
            column.ascending = records.iter().map(|record| record.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
            column.descending = records.iter().map(|record| record.slot).collect();
            column.descending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], true, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        }
        for composite in &mut sorting.composites {
            let columns = composite
                .fields
                .iter()
                .map(|field| {
                    sorting
                        .columns
                        .get(&field.path)
                        .map(|column| (column, field.descending))
                        .ok_or("composite sort column missing")
                })
                .collect::<Result<Vec<_>, _>>()?;
            composite.order = records.iter().map(|record| record.slot).collect();
            composite.order.sort_by(|a, b| {
                compare_composite(&columns, *a, *b)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        }
        sorting.restore_placement(placed_orders)?;
        let mut identity_order = records.iter().map(|record| record.slot).collect::<Vec<_>>();
        identity_order.sort_by(|a, b| records[a.0 as usize].id.cmp(&records[b.0 as usize].id));
        let inference = consolidate_inference(shards, &logical_records, records.len(), &offsets)?;
        let backings = shards
            .iter()
            .map(|shard| &shard.backing)
            .collect::<Vec<_>>();
        let backing = super::SegmentBacking::concatenate(&backings)?;
        let mut index = IndexedCollectionSegment {
            id: first.id,
            collection: first.collection,
            source_id: first.source_id.clone(),
            source_version: first.source_version,
            files,
            file_index,
            records,
            backing,
            write_versions: super::record_versions::RecordWriteVersions::concatenate(
                shards,
                total_records,
            )?,
            raw_covers_logical_fields: shards.iter().all(|shard| shard.raw_covers_logical_fields),
            id_lookup,
            identity_order,
            postings: postings.into(),
            inference,
            custom_postings,
            raw: raw.into(),
            fallback_text: fallback_text.into(),
            custom_text,
            logical_records,
            empty_array_records,
            sorting: sorting.into(),
            relations,
            record_parses,
            failed_parses,
            accounting: SegmentAccounting::default(),
        };
        index.recompute_accounting();
        Ok(ConsolidatedSegment { index, offsets })
    }
}
