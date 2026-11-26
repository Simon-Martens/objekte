//! Bounded slot blocks borrowed from native bitmap dictionaries.

use roaring::RoaringBitmap;

use crate::index::traits::transfer::StorageResult;
use crate::model::RecordSlot;

pub(super) fn visit_bitmap_blocks(
    bits: &RoaringBitmap,
    maximum: usize,
    mut visit: impl FnMut(&[RecordSlot]) -> StorageResult<()>,
) -> StorageResult<()> {
    if bits.is_empty() {
        return visit(&[]);
    }
    let mut batch = smallvec::SmallVec::<[RecordSlot; 256]>::new();
    for slot in bits {
        batch.push(RecordSlot(slot));
        if batch.len() == maximum {
            visit(&batch)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        visit(&batch)?;
    }
    Ok(())
}
