use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::index::storage::liveness::InitialLiveness;
use crate::inventory::{DiagnosticRef, RecordClass};
use crate::model::{RecordId, Value};
use crate::query::options::ReturnMode;
use crate::query_execution::runner::get_segment_record_by_id;
use crate::query_results::Payload;

/// Independent table with owned reads. Neither construction nor lookup uses
/// the memory segment header, backing columns, or ID hash table.
struct RecordTable {
    by_id: BTreeMap<RecordId, RecordSlot>,
    rows: Vec<(RecordHeader, Value)>,
    bodies_read: AtomicUsize,
    fail_read: AtomicBool,
    fail_header_after: Option<usize>,
}

impl SegmentReader for RecordTable {
    fn descriptor(&self) -> SegmentDescriptor {
        SegmentDescriptor {
            id: SegmentId::new(2),
            collection: CollectionId(0),
            slots: self.rows.len(),
        }
    }

    fn total_fallback_count(&self) -> u64 {
        self.rows
            .iter()
            .filter(|(header, _)| header.class == RecordClass::RawFallback)
            .count() as u64
    }

    fn active_fallback_count(&self, active: &ActiveRecords) -> StorageResult<u64> {
        self.rows.iter().try_fold(0, |count, (header, _)| {
            let fallback =
                header.class == RecordClass::RawFallback && active.contains(header.slot)?;
            Ok(count + u64::from(fallback))
        })
    }

    fn slot_members(&self, fact: SegmentSlotFact) -> StorageResult<SlotSet<'_>> {
        match fact {
            SegmentSlotFact::Logical => {
                let slots = self
                    .rows
                    .iter()
                    .filter(|(header, _)| header.class == RecordClass::Logical)
                    .map(|(header, _)| header.slot.0)
                    .collect::<Vec<_>>();
                SlotSet::from_sorted(slots.into()).map_err(|error| {
                    StorageError::invalid("read table slot facts", error.to_string())
                })
            }
            SegmentSlotFact::EmptyArray => Ok(SlotSet::Empty),
        }
    }

    fn source(&self) -> SegmentSource<'_> {
        SegmentSource {
            adapter: "fixture-record-table",
            version: 1,
            registration: Some(0),
        }
    }

    fn read_header_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordHeader]>, BatchStatus)> {
        const OP: &str = "read test headers";
        if self.fail_header_after.is_some_and(|after| start >= after) {
            return Err(StorageError::invalid(OP, "injected late header failure"));
        }
        let remaining = self
            .rows
            .get(start..)
            .ok_or_else(|| StorageError::invalid(OP, "start beyond last slot"))?;
        let mut batch = Vec::new();
        let mut bytes = 0;
        for (header, _) in remaining.iter().take(2) {
            let size = size_of::<RecordHeader>() + header.id.0.len();
            if !limit.accepts(OP, batch.len(), bytes, size)? {
                break;
            }
            bytes += size;
            batch.push(header.clone());
        }
        let status = if batch.len() == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Owned(batch), status))
    }

    fn backing_kind(&self) -> RecordBacking {
        RecordBacking::Retained
    }

    fn slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        Ok(self.by_id.get(id).copied())
    }

    fn header(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RecordHeader>>> {
        Ok(self
            .rows
            .get(slot.0 as usize)
            .map(|(header, _)| Cow::Owned(header.clone())))
    }

    fn origin(&self, slot: RecordSlot) -> StorageResult<Option<RecordOrigin>> {
        Ok(((slot.0 as usize) < self.rows.len()).then_some(RecordOrigin::Retained(None)))
    }

    fn retained_body(&self, slot: RecordSlot) -> StorageResult<Option<Cow<'_, RetainedBody>>> {
        self.bodies_read.fetch_add(1, Ordering::Relaxed);
        if self.fail_read.load(Ordering::Relaxed) {
            return Err(StorageError::invalid(
                "read record table",
                "injected body read failure",
            ));
        }
        Ok(self.rows.get(slot.0 as usize).map(|(_, value)| {
            Cow::Owned(RetainedBody {
                value: value.clone(),
                text: Vec::new(),
            })
        }))
    }

    fn file_metadata(&self, _file: FileId) -> StorageResult<Option<Cow<'_, FileEntry>>> {
        Ok(None)
    }

    fn visit_file_slots(
        &self,
        _file: FileId,
        _limit: BatchLimit,
        _visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        Ok(())
    }

    fn stored_key(
        &self,
        _path: &FieldPath,
        _slot: RecordSlot,
    ) -> StorageResult<Option<Cow<'_, SortKey>>> {
        Ok(None)
    }

    fn ordered_slots(
        &self,
        _sort: &[SortField],
        _scalar: Option<(&FieldPath, bool)>,
    ) -> StorageResult<Option<&dyn OrderedSlots>> {
        Ok(None)
    }
}

#[test]
fn real_direct_lookup_supports_owned_record_rows_and_checked_payload_reads() {
    let mut builder = crate::lifecycle::Index::builder();
    builder
        .collection("entries", crate::schema::Schema::default())
        .unwrap();
    let index = builder.build().unwrap();
    let view = index.view().unwrap();
    let record = Value::String("logical value".into());
    let id = RecordId::intrinsic("id").unwrap();
    let slot = RecordSlot(0);
    let segment = RecordTable {
        by_id: BTreeMap::from([(id.clone(), slot)]),
        rows: vec![(
            RecordHeader {
                slot,
                id,
                class: RecordClass::Logical,
                diagnostic_ref: DiagnosticRef(0),
            },
            record.clone(),
        )],
        bodies_read: AtomicUsize::new(0),
        fail_read: AtomicBool::new(false),
        fail_header_after: None,
    };
    let logical = read_slot_members(&segment, SegmentSlotFact::Logical).unwrap();
    assert_eq!(logical.view().iter().collect::<Vec<_>>(), [slot.0]);

    let active = crate::index::storage::test_factory()
        .live_records(1, InitialLiveness::All)
        .unwrap();
    let lookup = |id: &str, mode| {
        get_segment_record_by_id(
            view.repository(),
            0,
            &segment,
            &active,
            &view.0.catalog,
            view.revision(),
            id,
            mode,
        )
    };

    assert!(lookup("absent", ReturnMode::Locator).unwrap().is_none());
    let visible = lookup("id", ReturnMode::Locator).unwrap().unwrap();
    assert_eq!(visible.record_id, "id");
    assert!(visible.file.is_none());
    assert_eq!(segment.bodies_read.load(Ordering::Relaxed), 0);
    assert!(matches!(
        lookup("id", ReturnMode::Logical).unwrap().unwrap().payload,
        Some(Payload::Logical(value)) if value == record
    ));
    segment.fail_read.store(true, Ordering::Relaxed);
    assert!(
        lookup("id", ReturnMode::Logical)
            .unwrap_err()
            .contains("injected body read failure")
    );
}

#[test]
fn header_batches_preserve_slot_order_and_report_a_late_failure() {
    let rows: Vec<_> = (0..5)
        .map(|slot| {
            let id = RecordId::intrinsic(format!("row-{slot}")).unwrap();
            (
                RecordHeader {
                    slot: RecordSlot(slot),
                    id,
                    class: RecordClass::Logical,
                    diagnostic_ref: DiagnosticRef(0),
                },
                Value::Null,
            )
        })
        .collect();
    let mut segment = RecordTable {
        by_id: rows
            .iter()
            .map(|(header, _)| (header.id.clone(), header.slot))
            .collect(),
        rows,
        bodies_read: AtomicUsize::new(0),
        fail_read: AtomicBool::new(false),
        fail_header_after: None,
    };
    let collect = |reader: &RecordTable, slots: &mut Vec<u32>| {
        visit_record_headers(reader, |headers| {
            assert!(headers.len() <= 2);
            slots.extend(headers.iter().map(|header| header.slot.0));
            Ok::<_, StorageError>(())
        })
    };
    let mut slots = Vec::new();
    collect(&segment, &mut slots).unwrap();
    assert_eq!(slots, [0, 1, 2, 3, 4]);
    slots.clear();
    segment.fail_header_after = Some(2);
    let error = collect(&segment, &mut slots).unwrap_err();
    assert!(error.to_string().contains("injected late header failure"));
    assert_eq!(slots, [0, 1]);
    assert_eq!(segment.bodies_read.load(Ordering::Relaxed), 0);
}
