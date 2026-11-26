use roaring::RoaringBitmap;

use super::super::IndexedCollectionSegment;
use crate::inventory::ActiveRecords;
use crate::model::RecordSlot;

/// A partition reads one shared source-to-part routing table. It never
/// allocates or scans a source-sized slot table for each output shard.
pub(crate) struct PartitionRouting {
    pub(crate) locations: Vec<Option<(usize, RecordSlot)>>,
    pub(crate) selected: Vec<Vec<u32>>,
    pub(crate) logical_counts: Vec<u32>,
}

pub(super) fn scatter_bitmap_partitions(
    source: &RoaringBitmap,
    routing: &PartitionRouting,
) -> Result<Vec<RoaringBitmap>, String> {
    let mut parts = (0..routing.selected.len())
        .map(|_| RoaringBitmap::new())
        .collect::<Vec<_>>();
    for old in source.iter() {
        if let Some((part, slot)) = routing
            .locations
            .get(old as usize)
            .ok_or("domain slot outside source")?
        {
            parts[*part].insert(slot.0);
        }
    }
    Ok(parts)
}

impl PartitionRouting {
    pub(crate) fn new(
        source: &IndexedCollectionSegment,
        masks: &[ActiveRecords],
    ) -> Result<Self, String> {
        Self::from_masks(source.record_count(), masks.iter(), |slot| {
            source.logical_records.contains(slot)
        })
    }

    pub(crate) fn from_masks<'a>(
        source_slots: usize,
        masks: impl ExactSizeIterator<Item = &'a ActiveRecords>,
        is_logical: impl Fn(u32) -> bool,
    ) -> Result<Self, String> {
        let mut locations = vec![None; source_slots];
        let mut selected = Vec::with_capacity(masks.len());
        let mut logical_counts = Vec::with_capacity(masks.len());
        for (part, mask) in masks.enumerate() {
            if mask.slot_count() as usize != source_slots {
                return Err("partition posting mask coverage mismatch".into());
            }
            let mut slots = Vec::with_capacity(mask.len() as usize);
            let mut logical = 0u32;
            for old in mask.bitmap().iter() {
                let local = u32::try_from(slots.len()).map_err(|_| "partition slot overflow")?;
                let location = locations
                    .get_mut(old as usize)
                    .ok_or("partition posting slot outside source")?;
                if location.replace((part, RecordSlot(local))).is_some() {
                    return Err("partition posting slot selected twice".into());
                }
                slots.push(old);
                if is_logical(old) {
                    logical = logical
                        .checked_add(1)
                        .ok_or("partition logical count overflow")?;
                }
            }
            selected.push(slots);
            logical_counts.push(logical);
        }
        Ok(Self {
            locations,
            selected,
            logical_counts,
        })
    }
}

pub(crate) enum SlotMap<'a> {
    Dense(&'a [Option<RecordSlot>]),
    Partition {
        routing: &'a PartitionRouting,
        part: usize,
    },
}

impl SlotMap<'_> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Dense(slots) => slots.len(),
            Self::Partition { routing, .. } => routing.locations.len(),
        }
    }

    pub(crate) fn get(&self, old: usize) -> Option<Option<RecordSlot>> {
        match self {
            Self::Dense(slots) => slots.get(old).copied(),
            Self::Partition { routing, part } => routing.locations.get(old).map(|location| {
                location.and_then(|(owner, slot)| (owner == *part).then_some(slot))
            }),
        }
    }

    pub(crate) fn selected(&self) -> Box<dyn Iterator<Item = (usize, RecordSlot)> + '_> {
        match self {
            Self::Dense(slots) => Box::new(
                slots
                    .iter()
                    .enumerate()
                    .filter_map(|(source, destination)| destination.map(|slot| (source, slot))),
            ),
            Self::Partition { routing, part } => {
                Box::new(routing.selected[*part].iter().enumerate().map(
                    |(destination, &source)| (source as usize, RecordSlot(destination as u32)),
                ))
            }
        }
    }

    pub(crate) fn for_each_selected(
        &self,
        mut visit: impl FnMut(usize, RecordSlot) -> Result<(), String>,
    ) -> Result<(), String> {
        match self {
            Self::Dense(slots) => {
                for (old, slot) in slots.iter().enumerate() {
                    if let Some(slot) = slot {
                        visit(old, *slot)?;
                    }
                }
            }
            Self::Partition { routing, part } => {
                for &old in &routing.selected[*part] {
                    let slot = self
                        .get(old as usize)
                        .flatten()
                        .ok_or("partition routing missing")?;
                    visit(old as usize, slot)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn bounds(&self) -> Result<Option<(u32, u32)>, String> {
        let mut bounds: Option<(u32, u32)> = None;
        self.for_each_selected(|old, _| {
            let old = u32::try_from(old).map_err(|_| "compaction slot overflow")?;
            bounds = Some(match bounds {
                Some((first, last)) => (first.min(old), last.max(old)),
                None => (old, old),
            });
            Ok(())
        })?;
        Ok(bounds)
    }
}

pub(crate) fn project_bitmap(
    source: &RoaringBitmap,
    slots: &SlotMap<'_>,
) -> Result<RoaringBitmap, String> {
    project_bitmap_in_range(source, slots, None)
}

pub(crate) fn project_bitmap_in_range(
    source: &RoaringBitmap,
    slots: &SlotMap<'_>,
    bounds: Option<(u32, u32)>,
) -> Result<RoaringBitmap, String> {
    let mut projected = RoaringBitmap::new();
    let Some((first, last)) = bounds else {
        for old in source.iter() {
            if let Some(slot) = slots
                .get(old as usize)
                .ok_or("posting slot outside segment")?
            {
                projected.insert(slot.0);
            }
        }
        return Ok(projected);
    };
    for old in source.range(first..=last) {
        if let Some(slot) = slots
            .get(old as usize)
            .ok_or("posting slot outside segment")?
        {
            projected.insert(slot.0);
        }
    }
    Ok(projected)
}
