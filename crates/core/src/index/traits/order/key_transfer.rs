use std::borrow::Cow;
use std::num::NonZeroUsize;

use crate::index::traits::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
use crate::model::{Number, RecordSlot};

use super::{OrderColumnReader, SortKey};

const LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);
const OP: &str = "transfer declared order keys";

pub(super) fn key_bytes(key: &SortKey) -> usize {
    let variable = match key {
        SortKey::String(value) => value.len(),
        SortKey::Id(value) => value.len(),
        SortKey::Number(Number::ExactDecimal(value)) => value.digits.len(),
        _ => 0,
    };
    size_of::<SortKey>().saturating_add(variable)
}

pub(crate) fn read_order_keys<'a, C: OrderColumnReader + ?Sized>(
    source: &'a C,
    slots: &[RecordSlot],
    limit: BatchLimit,
) -> StorageResult<Vec<Cow<'a, SortKey>>> {
    let mut keys = Vec::new();
    let mut bytes = 0usize;
    for &slot in slots {
        if !limit.accepts(OP, keys.len(), bytes, 0)? {
            break;
        }
        let key = source.key(slot)?;
        let size = key_bytes(&key);
        if !limit.accepts(OP, keys.len(), bytes, size)? {
            break;
        }
        bytes += size;
        keys.push(key);
    }
    Ok(keys)
}

/// Deliver requested keys in input order while retaining at most one batch.
/// The sink can adopt owned keys or clone borrowed native values.
pub(crate) fn transfer_order_keys(
    source: &dyn OrderColumnReader,
    slots: impl IntoIterator<Item = StorageResult<RecordSlot>>,
    cancelled: &dyn Fn() -> bool,
    mut write: impl FnMut(SortKey) -> StorageResult<()>,
) -> StorageResult<()> {
    let count = LIMIT.fixed_batch_len(OP, size_of::<SortKey>())?;
    let mut slots = slots.into_iter();
    let mut requested = Vec::new();
    loop {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        requested.clear();
        for slot in slots.by_ref().take(count) {
            requested.push(slot?);
        }
        if requested.is_empty() {
            return Ok(());
        }
        let mut remaining = requested.as_slice();
        while !remaining.is_empty() {
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            let (keys, status) = source.key_batch(remaining, LIMIT)?;
            let consumed = keys.len();
            if consumed == 0
                || consumed > remaining.len()
                || (status == BatchStatus::Done) != (consumed == remaining.len())
            {
                return Err(StorageError::invalid(OP, "invalid key batch progress"));
            }
            let mut bytes = 0usize;
            for (index, key) in keys.into_iter().enumerate() {
                let size = key_bytes(&key);
                if !LIMIT.accepts(OP, index, bytes, size)? {
                    return Err(StorageError::invalid(OP, "key batch exceeded limits"));
                }
                bytes += size;
                write(key.into_owned())?;
            }
            remaining = &remaining[consumed..];
        }
    }
}
