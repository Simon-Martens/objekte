//! Import checked segment blocks into native columns.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::projection::{
    PartitionRouting, SlotMap, finish_projected_inference, transfer_bigram,
    transfer_bigram_partitions, transfer_posting, transfer_posting_partitions,
};
use super::record_versions::RecordWriteVersions;
use super::{
    IndexedCollectionSegment, MemoryRetainedBlock, RetainedAddress, SegmentBacking, SegmentFile,
};
use crate::index::memory::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::memory::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::memory::relation::RelationContribution;
use crate::index::storage::component::ComponentKind;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::segment::maintenance::SegmentPartition;
use crate::index::storage::segment::transfer::{FileSegmentLayout, read_retained_body};
use crate::index::storage::segment::{
    RecordBacking, RecordOrigin, SegmentHandle, SegmentReader, SegmentSlotFact, read_slot_members,
    visit_checked_file_slots,
};
use crate::index::storage::text::BigramAlphabet;
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inventory::RecordClass;

const OP: &str = "import memory segment";

fn invalid(detail: &'static str) -> StorageError {
    StorageError::invalid(OP, detail)
}

#[derive(Default)]
struct ScatteredColumns {
    postings: Vec<PostingIndex>,
    grams: Vec<BigramIndex>,
}

pub(crate) fn import_partitions(
    source: &dyn SegmentReader,
    partitions: &[SegmentPartition],
    catalog: &dyn FileCatalogReader,
    cancelled: &mut dyn FnMut() -> bool,
) -> StorageResult<Vec<SegmentHandle>> {
    let unsupported = || StorageError::Unsupported {
        operation: "import segment into memory partitions",
    };
    if partitions.is_empty() {
        return Ok(Vec::new());
    }
    let cancel = RefCell::new(cancelled);
    let check = || cancel.borrow_mut()();
    let logical = read_slot_members(source, SegmentSlotFact::Logical)?;
    let routing = PartitionRouting::from_masks(
        source.descriptor().slots,
        partitions.iter().map(|part| &part.slots),
        |slot| logical.view().contains(slot),
    )
    .map_err(|error| StorageError::invalid(OP, error))?;
    let mut columns = (0..partitions.len())
        .map(|_| ScatteredColumns::default())
        .collect::<Vec<_>>();
    for component in source.components()? {
        if check() {
            return Err(StorageError::Cancelled);
        }
        match component.kind {
            ComponentKind::DeclaredPosting
            | ComponentKind::CustomPosting
            | ComponentKind::InferredPosting => {
                let reader = source
                    .posting(&component)?
                    .ok_or_else(|| invalid("advertised posting missing"))?;
                if reader.descriptor().coverage.covered_records as usize != logical.view().len() {
                    return Err(unsupported());
                }
                for (part, column) in columns
                    .iter_mut()
                    .zip(transfer_posting_partitions(reader, &routing, &check)?)
                {
                    part.postings.push(column);
                }
            }
            ComponentKind::RawText | ComponentKind::FallbackText | ComponentKind::FieldText => {
                let reader = source
                    .grams(&component)?
                    .ok_or_else(|| invalid("advertised gram store missing"))?;
                if reader.descriptor().covered_records as usize != routing.locations.len() {
                    return Err(unsupported());
                }
                for (part, column) in columns
                    .iter_mut()
                    .zip(transfer_bigram_partitions(reader, &routing, &check)?)
                {
                    part.grams.push(column);
                }
            }
            ComponentKind::Relations | ComponentKind::Sorting => {}
        }
    }
    partitions
        .iter()
        .zip(columns)
        .enumerate()
        .map(|(part, (partition, columns))| {
            import_with_routing(
                source,
                FileSegmentLayout::Dense { partition, catalog },
                &mut || check(),
                Some((&routing, part, columns)),
            )?
            .ok_or_else(unsupported)
        })
        .collect()
}

pub(crate) fn import(
    source: &dyn SegmentReader,
    layout: FileSegmentLayout<'_>,
    cancelled: &mut dyn FnMut() -> bool,
) -> StorageResult<Option<SegmentHandle>> {
    import_with_routing(source, layout, cancelled, None)
}

fn import_with_routing(
    source: &dyn SegmentReader,
    layout: FileSegmentLayout<'_>,
    cancelled: &mut dyn FnMut() -> bool,
    scattered: Option<(&PartitionRouting, usize, ScatteredColumns)>,
) -> StorageResult<Option<SegmentHandle>> {
    let backing = source.backing_kind();
    let adapter = source.source();
    if (backing == RecordBacking::FileRange) != adapter.registration.is_none() {
        return Err(invalid("record backing and source registration disagree"));
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
    if backing == RecordBacking::FileRange && (!raw || !fallback) {
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
        placed: Default::default(),
        short_string_coverage: HashMap::new(),
    });
    if let Some(state) = &inference
        && (state.report.eligible_records != source_logical.len() as u64
            || state.report.unavailable_records != (descriptor.slots - source_logical.len()) as u64)
    {
        return Err(invalid("inference report domain mismatch"));
    }
    let cancelled = RefCell::new(cancelled);
    let cancelled_body = &cancelled;
    let mut cancelled = || cancelled_body.borrow_mut()();
    let empty_arrays = read_slot_members(source, SegmentSlotFact::EmptyArray)?;
    let empty_arrays = empty_arrays.view();
    let mut records = Vec::new();
    let mut locators = Vec::new();
    let mut bodies = Vec::new();
    let mut provider_versions = Vec::new();
    let mut write_versions = Vec::new();
    let mut logical = RoaringBitmap::new();
    let mut empty = RoaringBitmap::new();
    let mut mapping = if scattered.is_none() {
        vec![None; descriptor.slots]
    } else {
        Vec::new()
    };
    let mut id_lookup = HashMap::new();
    layout.visit_records(source, &mut cancelled, &mut |rows| {
        for row in rows {
            let header = row.header;
            let is_logical = header.class == RecordClass::Logical;
            match row.origin {
                RecordOrigin::FileRange(_) if row.write_version.is_none() => {}
                RecordOrigin::Retained(_) => {
                    bodies.push(read_retained_body(source, header.slot, &|| {
                        cancelled_body.borrow_mut()()
                    })?);
                }
                RecordOrigin::Provider {
                    registration,
                    version,
                    ..
                } if adapter.registration == Some(registration) => {
                    provider_versions.push(version);
                }
                _ => return Err(invalid("incompatible record origin")),
            }
            if backing != RecordBacking::FileRange {
                write_versions.push(
                    row.write_version
                        .ok_or_else(|| invalid("direct write version missing"))?,
                );
            }
            if source_logical.contains(header.slot.0) != is_logical
                || id_lookup
                    .insert(header.id.clone(), row.destination)
                    .is_some()
            {
                return Err(invalid("incompatible or duplicate record"));
            }
            if is_logical {
                logical.insert(row.destination.0);
            }
            if empty_arrays.contains(header.slot.0) {
                empty.insert(row.destination.0);
            }
            let mut header = header.clone();
            if let Some((routing, part, _)) = &scattered {
                if routing.locations[header.slot.0 as usize] != Some((*part, row.destination)) {
                    return Err(invalid("record header and partition routing disagree"));
                }
            } else {
                mapping[header.slot.0 as usize] = Some(row.destination);
            }
            header.slot = row.destination;
            records.push(header);
            locators.push(row.origin.file());
        }
        Ok(())
    })?;
    let (mapping, columns) = if let Some((routing, part, columns)) = scattered {
        (SlotMap::Partition { routing, part }, Some(columns))
    } else {
        (SlotMap::Dense(&mapping), None)
    };
    let (mut scattered_postings, mut scattered_grams) = columns.map_or((None, None), |columns| {
        (
            Some(columns.postings.into_iter()),
            Some(columns.grams.into_iter()),
        )
    });
    let mut files = Vec::new();
    let mut relocated = HashSet::new();
    let mut file_index = HashMap::new();
    let mut seen = RoaringBitmap::new();
    layout.visit_files(source, &mut cancelled, |file| {
        let mut slots = Vec::new();
        visit_checked_file_slots(source, file.id, &mut |batch| {
            for slot in batch {
                let Some(destination) = mapping.get(slot.0 as usize).flatten() else {
                    continue;
                };
                let locator = locators[destination.0 as usize]
                    .ok_or_else(|| invalid("file membership has no record link"))?;
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
                    diagnostic.record = mapping
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
    if backing == RecordBacking::FileRange && seen.len() as usize != records.len() {
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
            let file = locators[slot.0 as usize]?.file;
            relocated
                .contains(&file)
                .then(|| files[file_index[&file]].entry.relative_path.as_path())
        },
        &cancelled,
    )?;
    let mut declared = PostingCollection {
        fields: HashMap::new(),
    };
    for (component, reader) in postings {
        let covered_records = if dense {
            logical.len() as u32
        } else {
            reader.descriptor().coverage.covered_records
        };
        let column = if let Some(scattered) = &mut scattered_postings {
            scattered
                .next()
                .ok_or_else(|| invalid("scattered posting column missing"))?
        } else {
            transfer_posting(reader, &mapping, covered_records, None, &cancelled)?
        };
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
        let column = if let Some(scattered) = &mut scattered_grams {
            scattered
                .next()
                .ok_or_else(|| invalid("scattered gram column missing"))?
        } else {
            transfer_bigram(reader, &mapping, covered_records, None, &cancelled)?
        };
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
    let backing = match backing {
        RecordBacking::FileRange => SegmentBacking::FileRange {
            locators: locators
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| invalid("missing file origin"))?,
        },
        RecordBacking::Retained => SegmentBacking::Retained {
            registration: adapter.registration.expect("checked registration"),
            addresses: (0..bodies.len() as u32)
                .map(|offset| RetainedAddress { block: 0, offset })
                .collect(),
            blocks: vec![Arc::new(MemoryRetainedBlock(bodies))],
            links: locators,
        },
        RecordBacking::Provider => SegmentBacking::Provider {
            registration: adapter.registration.expect("checked registration"),
            versions: provider_versions,
            links: locators,
        },
    };
    let versions = if matches!(backing, SegmentBacking::FileRange { .. }) {
        RecordWriteVersions::Unversioned
    } else {
        RecordWriteVersions::Projected(write_versions.into())
    };
    let empty_grams = |alphabet| BigramIndex {
        alphabet,
        record_count: 0,
        normal: HashMap::new(),
        skip: HashMap::new(),
    };
    let alphabet = raw
        .as_ref()
        .or(fallback.as_ref())
        .map_or(BigramAlphabet::Ascii, |column| column.alphabet);
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
        backing,
        write_versions: versions,
        raw_covers_logical_fields: source.raw_covers_logical_fields(),
        id_lookup,
        identity_order,
        postings: declared.into(),
        placed_postings: Default::default(),
        inference,
        custom_postings: named_postings,
        raw: raw.unwrap_or_else(|| empty_grams(alphabet)).into(),
        placed_grams: Default::default(),
        fallback_text: fallback.unwrap_or_else(|| empty_grams(alphabet)).into(),
        custom_text,
        logical_records: logical,
        empty_array_records: empty,
        sorting: sorting.into(),
        relations,
        placed_relations: Default::default(),
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
