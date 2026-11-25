use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::index::storage::liveness::InitialLiveness;
use crate::index::storage::transfer::StorageError;
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
}

impl SegmentReader for RecordTable {
    fn descriptor(&self) -> SegmentDescriptor {
        SegmentDescriptor {
            id: SegmentId::new(2),
            collection: CollectionId(0),
            slots: self.rows.len(),
        }
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
    };
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
