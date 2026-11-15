//! Publication-specific record visibility persisted beside immutable runs.

use roaring::RoaringBitmap;

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
use super::format::{ObjectBuilder, ObjectKind, ObjectView, SectionKind};
use super::record_set::{RecordSetRef, RecordSetView, encode_record_set};
use super::store::ObjectInput;


/// One opened visibility object bound to a run slot domain.
pub struct VisibilityView {
    backing: Backing,
    reference: RecordSetRef,
    arena_offset: u64,
    arena_len: u64,
    slot_count: u32,
}


impl VisibilityView {
    pub fn open(backing: Backing, slot_count: u32) -> StorageResult<Self> {
        let view = ObjectView::new(&backing)?;
        if view.header().kind != ObjectKind::Visibility {
            return Err(StorageError::Incompatible(
                "visibility object has the wrong object kind".into(),
            ));
        }
        let section = view.required_section(SectionKind::Visibility)?;
        if section.length < super::record_set::RECORD_SET_DESCRIPTOR_LEN as u64 {
            return Err(StorageError::corrupt("visibility section is truncated"));
        }
        let descriptor = backing.read(
            section.offset,
            super::record_set::RECORD_SET_DESCRIPTOR_LEN as u64,
        )?;
        let reference = RecordSetRef::decode(descriptor)?;
        if reference.cardinality > slot_count as u64 {
            return Err(StorageError::corrupt(
                "visibility cardinality exceeds the run slot domain",
            ));
        }
        let arena_len = section
            .length
            .checked_sub(super::record_set::RECORD_SET_DESCRIPTOR_LEN as u64)
            .ok_or_else(|| StorageError::corrupt("visibility arena length underflow"))?;
        Ok(Self {
            backing,
            reference,
            arena_offset: section.offset + super::record_set::RECORD_SET_DESCRIPTOR_LEN as u64,
            arena_len,
            slot_count,
        })
    }

    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub fn cardinality(&self) -> u64 {
        self.reference.cardinality
    }

    pub fn view(&self) -> StorageResult<RecordSetView<'_>> {
        let arena = self.backing.read(self.arena_offset, self.arena_len)?;
        self.reference.view(arena, self.slot_count)
    }

    pub fn bitmap(&self) -> StorageResult<RoaringBitmap> {
        if self.reference.tag == super::record_set::RecordSetTag::All {
            let mut bitmap = RoaringBitmap::new();
            bitmap.insert_range(0..self.slot_count);
            return Ok(bitmap);
        }
        let view = self.view()?;
        let mut bitmap = RoaringBitmap::new();
        for slot in view.ids() {
            bitmap.insert(slot?);
        }
        Ok(bitmap)
    }

    pub fn is_full(&self) -> bool {
        self.reference.cardinality == self.slot_count as u64
    }
}


/// Build one small immutable visibility object for a run slot domain.
pub fn build_visibility(active: &RoaringBitmap, slot_count: u32) -> StorageResult<ObjectInput> {
    if active.max().is_some_and(|slot| slot >= slot_count) {
        return Err(StorageError::InvalidArgument(
            "visibility slot is outside the run domain".into(),
        ));
    }
    let mut section = Vec::new();
    if active.len() == slot_count as u64 {
        section.extend_from_slice(&RecordSetRef::all(slot_count).encode());
    } else {
        let slots = active.iter().collect::<Vec<_>>();
        let mut encoded = encode_record_set(&slots, slot_count)?;
        if encoded.reference.tag == super::record_set::RecordSetTag::Blocks {
            encoded.reference.a = encoded
                .reference
                .a
                .checked_add(super::record_set::RECORD_SET_DESCRIPTOR_LEN as u64)
                .ok_or_else(|| StorageError::ResourceLimit("visibility arena overflow".into()))?;
        }
        section.extend_from_slice(&encoded.reference.encode());
        section.extend_from_slice(&encoded.arena);
    }
    let mut builder = ObjectBuilder::new(ObjectKind::Visibility);
    builder.add_section(SectionKind::Visibility, section)?;
    let object = builder.finish()?;
    Ok(ObjectInput {
        kind: ObjectKind::Visibility,
        bytes: std::sync::Arc::from(object.into_bytes()),
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_objects_round_trip_sparse_and_full_domains() {
        for selected in [vec![], vec![0u32, 2, 5], (0..6).collect::<Vec<_>>()] {
            let active = selected.iter().copied().collect::<RoaringBitmap>();
            let input = build_visibility(&active, 6).unwrap();
            let view = VisibilityView::open(Backing::resident(input.bytes), 6).unwrap();
            assert_eq!(view.bitmap().unwrap(), active);
            assert_eq!(view.cardinality(), active.len());
        }
    }

    #[test]
    fn visibility_bytes_are_deterministic() {
        let active = RoaringBitmap::from_iter([0_u32]);
        let first = build_visibility(&active, 1).unwrap();
        let second = build_visibility(&active, 1).unwrap();
        assert_eq!(first.bytes.as_ref(), second.bytes.as_ref());
    }

    #[test]
    fn visibility_rejects_out_of_range_members() {
        let active = RoaringBitmap::from_iter([7_u32]);
        assert!(build_visibility(&active, 7).is_err());
    }
}
