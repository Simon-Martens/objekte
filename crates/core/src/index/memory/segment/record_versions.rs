use std::ops::Range;
use std::sync::Arc;

use crate::index::traits::segment::RecordWriteVersion;
use crate::model::RecordSlot;

use super::IndexedCollectionSegment;
use super::projection::SlotMap;

#[derive(Clone, Default)]
pub(crate) enum RecordWriteVersions {
    #[default]
    Unversioned,
    Contiguous(Range<u64>),
    Projected(Arc<[RecordWriteVersion]>),
}

impl RecordWriteVersions {
    pub(super) fn new(len: usize) -> Result<Self, String> {
        RecordWriteVersion::allocate(len)
            .map(Self::Contiguous)
            .map_err(String::from)
    }

    pub(super) fn get(&self, slot: RecordSlot) -> Option<RecordWriteVersion> {
        match self {
            Self::Unversioned => None,
            Self::Contiguous(range) => range
                .start
                .checked_add(slot.0 as u64)
                .filter(|value| *value < range.end)
                .and_then(RecordWriteVersion::new),
            Self::Projected(versions) => versions.get(slot.0 as usize).copied(),
        }
    }

    pub(super) fn project(&self, slots: &SlotMap<'_>, len: usize) -> Result<Self, String> {
        if matches!(self, Self::Unversioned) {
            return Ok(Self::Unversioned);
        }
        let mut projected = vec![None; len];
        slots.for_each_selected(|old, new| {
            let version = self
                .get(RecordSlot(old as u32))
                .ok_or("record write version missing")?;
            let destination = projected
                .get_mut(new.0 as usize)
                .ok_or("record write version slot outside projection")?;
            if destination.replace(version).is_some() {
                return Err("duplicate record write version slot".into());
            }
            Ok(())
        })?;
        let versions = projected
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("record write version projection is incomplete")?;
        Ok(Self::Projected(versions.into()))
    }

    pub(super) fn concatenate(
        shards: &[&IndexedCollectionSegment],
        len: usize,
    ) -> Result<Self, String> {
        if matches!(shards[0].write_versions, Self::Unversioned) {
            if shards
                .iter()
                .all(|shard| matches!(shard.write_versions, Self::Unversioned))
            {
                return Ok(Self::Unversioned);
            }
            return Err("incompatible record write versions".into());
        }
        let mut versions = Vec::with_capacity(len);
        for shard in shards {
            for slot in 0..shard.record_count() {
                versions.push(
                    shard
                        .write_versions
                        .get(RecordSlot(slot as u32))
                        .ok_or("record write version missing")?,
                );
            }
        }
        Ok(Self::Projected(versions.into()))
    }

    pub(super) fn resident_bytes(&self) -> usize {
        match self {
            Self::Projected(versions) => std::mem::size_of_val(versions.as_ref()),
            _ => 0,
        }
    }
}
