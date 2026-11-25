//! Fallible ordered traversal over a pinned component.

use std::borrow::Cow;
use std::mem::size_of;
use std::num::NonZeroUsize;

use crate::model::RecordSlot;

use super::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};

const ORDER_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(256).unwrap(),
    NonZeroUsize::new(256 * size_of::<RecordSlot>()).unwrap(),
);
const OP: &str = "read ordered slots";

pub(crate) trait OrderedSlots: Send + Sync {
    fn len(&self) -> usize;

    fn read_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordSlot]>, BatchStatus)>;
}

/// One cursor holds at most one batch. Native orders can lend a bounded view
/// of the resident array while another backend can return a decoded page.
pub(crate) struct OrderedSlotCursor<'a> {
    source: &'a dyn OrderedSlots,
    batch: Cow<'a, [RecordSlot]>,
    end: usize,
    position: usize,
    status: BatchStatus,
}

impl<'a> OrderedSlotCursor<'a> {
    pub(crate) fn new(source: &'a dyn OrderedSlots) -> Self {
        Self {
            source,
            batch: Cow::Borrowed(&[]),
            end: 0,
            position: 0,
            status: BatchStatus::More,
        }
    }

    pub(crate) fn owned_batch_bytes(&self) -> usize {
        match &self.batch {
            Cow::Borrowed(_) => 0,
            Cow::Owned(slots) => slots.capacity() * size_of::<RecordSlot>(),
        }
    }

    pub(crate) fn next_slot(&mut self) -> StorageResult<Option<RecordSlot>> {
        if self.position == self.batch.len() {
            if self.status == BatchStatus::Done {
                return Ok(None);
            }
            self.refill()?;
        }
        let slot = self.batch.get(self.position).copied();
        self.position += usize::from(slot.is_some());
        Ok(slot)
    }

    fn refill(&mut self) -> StorageResult<()> {
        self.position = 0;
        self.batch = Cow::Borrowed(&[]);
        let (batch, status) = self.source.read_batch(self.end, ORDER_LIMIT)?;
        let max_slots = ORDER_LIMIT.fixed_batch_len(OP, size_of::<RecordSlot>())?;
        if batch.len() > max_slots
            || matches!(&batch, Cow::Owned(slots) if slots.capacity() > max_slots)
        {
            return Err(StorageError::invalid(OP, "source exceeded batch limits"));
        }
        let end = self
            .end
            .checked_add(batch.len())
            .ok_or_else(|| StorageError::invalid(OP, "slot position overflow"))?;
        let complete = end == self.source.len();
        if end > self.source.len()
            || (status == BatchStatus::Done) != complete
            || (status == BatchStatus::More && batch.is_empty())
        {
            return Err(StorageError::invalid(
                OP,
                "source did not make bounded progress",
            ));
        }
        self.end = end;
        self.batch = batch;
        self.status = status;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ShortOrder {
        rows: [RecordSlot; 3],
        reads: AtomicUsize,
        fail: bool,
    }

    impl OrderedSlots for ShortOrder {
        fn len(&self) -> usize {
            self.rows.len()
        }

        fn read_batch(
            &self,
            start: usize,
            limit: BatchLimit,
        ) -> StorageResult<(Cow<'_, [RecordSlot]>, BatchStatus)> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            assert!(limit.fixed_batch_len(OP, size_of::<RecordSlot>())? >= 2);
            if self.fail && start > 0 {
                return Err(StorageError::invalid(OP, "test page read failed"));
            }
            let end = (start + 2).min(self.rows.len());
            let status = if end == self.rows.len() {
                BatchStatus::Done
            } else {
                BatchStatus::More
            };
            Ok((Cow::Owned(self.rows[start..end].to_vec()), status))
        }
    }

    #[test]
    fn a_short_page_stops_early_and_later_read_errors_surface() {
        let source = ShortOrder {
            rows: [RecordSlot(2), RecordSlot(0), RecordSlot(1)],
            reads: AtomicUsize::new(0),
            fail: true,
        };
        let mut cursor = OrderedSlotCursor::new(&source);
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
        assert_eq!(cursor.next_slot().unwrap(), Some(RecordSlot(2)));
        assert_eq!(cursor.next_slot().unwrap(), Some(RecordSlot(0)));
        assert_eq!(source.reads.load(Ordering::Relaxed), 1);
        assert!(
            cursor
                .next_slot()
                .unwrap_err()
                .to_string()
                .contains("test page read failed")
        );
        assert_eq!(source.reads.load(Ordering::Relaxed), 2);
    }
}
