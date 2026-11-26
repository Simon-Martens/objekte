//! Import checked file-backed segment blocks into native columns.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::projection::{SlotMap, finish_projected_inference, transfer_bigram, transfer_posting};
use super::{IndexedCollectionSegment, SegmentBacking, SegmentFile};
use crate::index::memory::inference::InferenceState;
use crate::index::memory::posting::{CustomPostingIndex, PostingCollection};
use crate::index::memory::relation::RelationContribution;
use crate::index::storage::component::ComponentKind;
use crate::index::storage::segment::transfer::FileSegmentLayout;
use crate::index::storage::segment::{
    RecordBacking, RecordOrigin, SegmentHandle, SegmentReader, SegmentSlotFact, read_slot_members,
    visit_checked_file_slots,
};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inventory::RecordClass;

const OP: &str = "import memory file segment";

fn invalid(detail: &'static str) -> StorageError {
    StorageError::invalid(OP, detail)
}

pub(crate) fn import(
    source: &dyn SegmentReader,
    layout: FileSegmentLayout<'_>,
    cancelled: &mut dyn FnMut() -> bool,
) -> StorageResult<Option<SegmentHandle>> {
    if source.backing_kind() != RecordBacking::FileRange || source.source().registration.is_some() {
        return Ok(None);
    }
    let descriptor = source.descriptor();
    let components = source.components()?;
    let order_columns = source.order_columns()?;
    let composite_orders = source.composite_orders()?;
    let mut postings = Vec::new();
    let mut named_postings = HashMap::new();
    let mut grams = Vec::new();
    let mut raw = false;
    let mut fallback = false;
    let mut relation_family = false;
    let mut sorting_family = false;
    let source_logical = read_slot_members(source, SegmentSlotFact::Logical)?;
    let source_logical = source_logical.view();
    let dense = matches!(layout, FileSegmentLayout::Dense { .. });
    for component in &components {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        if component.collection != descriptor.collection || component.segment != descriptor.id {
            return Err(invalid("component belongs to a different segment"));
        }
        match component.kind {
            ComponentKind::DeclaredPosting
            | ComponentKind::CustomPosting
            | ComponentKind::InferredPosting => {
                let reader = source
                    .posting(component)?
                    .ok_or_else(|| invalid("advertised posting missing"))?;
                let posting = reader.descriptor();
                if component.path.as_deref() != Some(posting.path)
                    || component.binding.is_some()
                        != (component.kind == ComponentKind::CustomPosting)
                    || posting.coverage.covered_records as usize > source_logical.len()
                {
                    return Err(invalid("posting descriptor mismatch"));
                }
                if dense && posting.coverage.covered_records as usize != source_logical.len() {
                    return Ok(None);
                }
                postings.push((component, reader));
            }
            ComponentKind::RawText | ComponentKind::FallbackText | ComponentKind::FieldText => {
                let reader = source
                    .grams(component)?
                    .ok_or_else(|| invalid("advertised gram store missing"))?;
                match component.kind {
                    ComponentKind::RawText if !raw => raw = true,
                    ComponentKind::FallbackText if !fallback => fallback = true,
                    ComponentKind::FieldText
                        if component.path.is_some() && component.binding.is_some() => {}
                    _ => return Err(invalid("invalid gram binding")),
                }
                if dense && reader.descriptor().covered_records as usize != descriptor.slots {
                    return Ok(None);
                }
                grams.push((component, reader));
            }
            ComponentKind::Sorting => {
                if sorting_family || component.path.is_some() || component.binding.is_some() {
                    return Err(invalid("invalid sorting descriptor"));
                }
                sorting_family = true;
            }
            ComponentKind::Relations => {
                if relation_family || component.path.is_some() || component.binding.is_some() {
                    return Err(invalid("invalid relation descriptor"));
                }
                relation_family = true;
            }
        }
    }
    if !raw || !fallback {
        return Ok(None);
    }
    if (!order_columns.is_empty() || !composite_orders.is_empty()) && !sorting_family {
        return Err(invalid("missing sorting descriptor"));
    }
    if source.relation_count() != 0 && !relation_family {
        return Err(invalid("missing relation descriptor"));
    }
    let mut inference = source.inference_report()?.map(|report| InferenceState {
        report: report.into_owned(),
        indexes: HashMap::new(),
        short_string_coverage: HashMap::new(),
    });
    if let Some(state) = &inference
        && (state.report.eligible_records != source_logical.len() as u64
            || state.report.unavailable_records != (descriptor.slots - source_logical.len()) as u64)
    {
        return Err(invalid("inference report domain mismatch"));
    }
    let cancelled = RefCell::new(cancelled);
    let mut cancelled = || cancelled.borrow_mut()();
    let empty_arrays = read_slot_members(source, SegmentSlotFact::EmptyArray)?;
    let empty_arrays = empty_arrays.view();
    let mut records = Vec::new();
    let mut locators = Vec::new();
    let mut logical = RoaringBitmap::new();
    let mut empty = RoaringBitmap::new();
    let mut mapping = vec![None; descriptor.slots];
    let mut id_lookup = HashMap::new();
    layout.visit_records(source, &mut cancelled, &mut |rows| {
        for row in rows {
            let RecordOrigin::FileRange(locator) = row.origin else {
                return Err(invalid("expected file-backed origin"));
            };
            let header = row.header;
            let is_logical = header.class == RecordClass::Logical;
            if row.write_version.is_some()
                || source_logical.contains(header.slot.0) != is_logical
                || id_lookup
                    .insert(header.id.clone(), row.destination)
                    .is_some()
            {
                return Err(invalid("incompatible or duplicate file record"));
            }
            if is_logical {
                logical.insert(row.destination.0);
            }
            if empty_arrays.contains(header.slot.0) {
                empty.insert(row.destination.0);
            }
            let mut header = header.clone();
            mapping[header.slot.0 as usize] = Some(row.destination);
            header.slot = row.destination;
            records.push(header);
            locators.push(locator);
        }
        Ok(())
    })?;
    let mut files = Vec::new();
    let mut relocated = HashSet::new();
    let mut file_index = HashMap::new();
    let mut seen = RoaringBitmap::new();
    layout.visit_files(source, &mut cancelled, |file| {
        let mut slots = Vec::new();
        visit_checked_file_slots(source, file.id, &mut |batch| {
            for slot in batch {
                let Some(destination) = mapping[slot.0 as usize] else {
                    continue;
                };
                let locator = locators[destination.0 as usize];
                if locator.file != file.id
                    || locator.revision != file.revision
                    || slots
                        .last()
                        .is_some_and(|previous| previous >= &destination)
                    || !seen.insert(destination.0)
                {
                    return Err(invalid("file membership and record origin disagree"));
                }
                slots.push(destination);
            }
            Ok(())
        })?;
        if file_index.insert(file.id, files.len()).is_some() {
            return Err(invalid("duplicate file row"));
        }
        let mut entry = file.clone();
        if dense {
            if !order_columns.is_empty() {
                let previous = source
                    .file_metadata(file.id)?
                    .ok_or_else(|| invalid("missing sort file metadata"))?;
                if previous.relative_path != file.relative_path {
                    relocated.insert(file.id);
                }
            }
            for diagnostic in &mut entry.diagnostics {
                if let Some(slot) = diagnostic.record {
                    diagnostic.record = *mapping
                        .get(slot.0 as usize)
                        .ok_or_else(|| invalid("diagnostic references an invalid slot"))?;
                }
            }
        }
        files.push(SegmentFile {
            entry,
            records: slots,
        });
        Ok(())
    })?;
    if seen.len() as usize != records.len() {
        return Err(invalid("file membership has incomplete coverage"));
    }

    let relations = (0..source.relation_count())
        .map(|ordinal| {
            let reader = source
                .relation(ordinal)?
                .ok_or_else(|| invalid("advertised relation missing"))?;
            RelationContribution::transfer_reader(reader, &mapping, records.len(), &cancelled)
        })
        .collect::<StorageResult<Vec<_>>>()?;

    let sorting = crate::index::memory::sorting::import::transfer(
        order_columns,
        composite_orders,
        &mapping,
        &records,
        |slot| {
            let file = locators[slot.0 as usize].file;
            relocated
                .contains(&file)
                .then(|| files[file_index[&file]].entry.relative_path.as_path())
        },
        &cancelled,
    )?;
    let mut declared = PostingCollection {
        fields: HashMap::new(),
    };
    let mapping = SlotMap::Dense(&mapping);
    for (component, reader) in postings {
        let covered_records = if dense {
            logical.len() as u32
        } else {
            reader.descriptor().coverage.covered_records
        };
        let column = transfer_posting(reader, &mapping, covered_records, None, &cancelled)?;
        let path = column.path.clone();
        let duplicate = match component.kind {
            ComponentKind::DeclaredPosting => declared.fields.insert(path, column).is_some(),
            ComponentKind::CustomPosting => {
                let complete = source
                    .named_posting_complete(component)?
                    .ok_or_else(|| invalid("named posting completeness missing"))?;
                let binding = component
                    .binding
                    .as_deref()
                    .expect("checked binding")
                    .to_owned();
                named_postings
                    .insert(
                        (path, binding),
                        CustomPostingIndex {
                            index: column,
                            complete,
                        },
                    )
                    .is_some()
            }
            ComponentKind::InferredPosting => {
                let state = inference
                    .as_mut()
                    .ok_or_else(|| invalid("inferred posting report missing"))?;
                if let Some(covered) = source.inferred_short_string_members(&path)? {
                    let slots = covered
                        .explicit_view()
                        .ok_or_else(|| invalid("unbounded short-string membership"))?;
                    let mut projected = RoaringBitmap::new();
                    for slot in slots.iter() {
                        if cancelled() {
                            return Err(StorageError::Cancelled);
                        }
                        if !source_logical.contains(slot) {
                            return Err(invalid("short-string membership outside logical domain"));
                        }
                        if let Some(destination) = mapping.get(slot as usize).flatten() {
                            projected.insert(destination.0);
                        }
                    }
                    if !projected.is_empty() || !dense {
                        state.short_string_coverage.insert(path.clone(), projected);
                    }
                }
                state.indexes.insert(path, column).is_some()
            }
            _ => unreachable!("validated posting family"),
        };
        if duplicate {
            return Err(invalid("duplicate posting binding"));
        }
    }
    if let Some(state) = inference.as_mut() {
        if state.report.admitted_paths != state.indexes.len() {
            return Err(invalid(
                "inference report describes a different dictionary set",
            ));
        }
        if dense {
            finish_projected_inference(state, logical.len(), records.len())
                .map_err(|error| StorageError::invalid(OP, error))?;
        }
    }
    let mut raw = None;
    let mut fallback = None;
    let mut custom_text = HashMap::new();
    for (component, reader) in grams {
        let covered_records = if dense {
            records.len() as u32
        } else {
            reader.descriptor().covered_records
        };
        let column = transfer_bigram(reader, &mapping, covered_records, None, &cancelled)?;
        match component.kind {
            ComponentKind::RawText => raw = Some(column),
            ComponentKind::FallbackText => fallback = Some(column),
            ComponentKind::FieldText => {
                let key = (
                    component.path.as_deref().expect("checked path").to_vec(),
                    component
                        .binding
                        .as_deref()
                        .expect("checked binding")
                        .to_owned(),
                );
                if custom_text.insert(key, column).is_some() {
                    return Err(invalid("duplicate gram binding"));
                }
            }
            _ => unreachable!("validated gram family"),
        }
    }
    let adapter = source.source();
    let mut identity_order = records.iter().map(|header| header.slot).collect::<Vec<_>>();
    identity_order.sort_unstable_by(|left, right| {
        records[left.0 as usize]
            .id
            .cmp(&records[right.0 as usize].id)
    });
    let mut segment = IndexedCollectionSegment {
        id: descriptor.id,
        collection: descriptor.collection,
        source_id: adapter.adapter.to_owned(),
        source_version: adapter.version,
        files,
        file_index,
        records,
        backing: SegmentBacking::FileRange { locators },
        write_versions: Default::default(),
        raw_covers_logical_fields: source.raw_covers_logical_fields(),
        id_lookup,
        identity_order,
        postings: declared.into(),
        inference,
        custom_postings: named_postings,
        raw: raw.expect("required raw store").into(),
        fallback_text: fallback.expect("required fallback store").into(),
        custom_text,
        logical_records: logical,
        empty_array_records: empty,
        sorting: sorting.into(),
        relations,
        record_parses: 0,
        failed_parses: 0,
        accounting: Default::default(),
    };
    segment.recompute_accounting();
    Ok(Some(Arc::new(segment)))
}

#[cfg(test)]
#[path = "import.test.rs"]
mod tests;
