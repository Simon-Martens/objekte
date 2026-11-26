use std::collections::BTreeMap;

use super::*;
use crate::lifecycle::Index;
use crate::model::Value;
use crate::query::options::ReturnMode;
use crate::records::RetainedBody;

#[derive(Default)]
struct TransferTable {
    headers: BTreeMap<RecordSlot, (RecordHeader, bool, Option<RecordWriteVersion>)>,
    bodies: BTreeMap<RecordSlot, RetainedBody>,
    current: Vec<u8>,
    part_count: usize,
    fail_after_part: Option<usize>,
}

impl RecordTransferSink for TransferTable {
    fn records(&mut self, records: &[RecordTransferRow<'_>]) -> StorageResult<()> {
        assert!(records.len() <= 128);
        for row in records {
            assert!(matches!(row.origin, RecordOrigin::Retained(None)));
            let mut header = row.header.clone();
            header.slot = row.destination;
            assert!(
                self.headers
                    .insert(row.destination, (header, row.live, row.write_version))
                    .is_none()
            );
        }
        Ok(())
    }

    fn wants_retained_bodies(&self) -> bool {
        true
    }

    fn retained_part(
        &mut self,
        slot: RecordSlot,
        version: u32,
        part: &[u8],
        last: bool,
    ) -> StorageResult<()> {
        assert_eq!(version, RETAINED_BODY_TRANSFER_VERSION);
        assert!(part.len() <= BODY_PART_BYTES);
        assert!(self.headers.contains_key(&slot));
        if self.fail_after_part == Some(self.part_count) {
            return Err(StorageError::invalid(
                "store transfer part",
                "injected late failure",
            ));
        }
        self.part_count += 1;
        self.current.extend_from_slice(part);
        if last {
            let (value, text) = bincode::deserialize(&self.current).map_err(|error| {
                StorageError::invalid("decode retained transfer body", error.to_string())
            })?;
            assert!(
                self.bodies
                    .insert(slot, RetainedBody { value, text })
                    .is_none()
            );
            self.current.clear();
        }
        Ok(())
    }
}

#[test]
fn retained_transfer_preserves_parts_liveness_and_write_versions() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Default::default()).unwrap();
    let registration = builder
        .register_records(collection, "transfer-retained", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    batch.insert("retired", Value::Null).unwrap();
    let text = "ordered retained body".repeat(10_000);
    batch
        .insert("retained", Value::String(text.clone()))
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let prior = index.view().unwrap();
    let mut batch = writer.batch();
    batch.delete("retired").unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let group = pinned.segments(collection).unwrap();
    let source = group[0].as_ref();
    let version = source.write_version(RecordSlot(1)).unwrap().unwrap();
    let mut identity = TransferTable::default();
    transfer_records(
        source,
        &source.active,
        RecordLayout::Identity,
        &mut || false,
        &mut identity,
    )
    .unwrap();
    assert_eq!(identity.headers.len(), 2);
    assert!(!identity.headers[&RecordSlot(0)].1);
    assert!(identity.headers[&RecordSlot(1)].1);
    assert_eq!(identity.headers[&RecordSlot(1)].2, Some(version));
    assert!(identity.part_count >= 4);
    assert_eq!(
        identity.bodies[&RecordSlot(1)].value,
        Value::String(text.clone())
    );
    let mut failed = TransferTable {
        fail_after_part: Some(1),
        ..Default::default()
    };
    let error = transfer_records(
        source,
        &source.active,
        RecordLayout::Dense,
        &mut || false,
        &mut failed,
    )
    .unwrap_err();
    assert!(error.to_string().contains("injected late failure"));
    assert_eq!(failed.part_count, 1);
    assert!(failed.bodies.is_empty());
    assert_eq!(index.view().unwrap().revision(), pinned.revision());
    assert!(
        prior
            .get_record("entries", "retired", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    let mut dense = TransferTable::default();
    transfer_records(
        source,
        &source.active,
        RecordLayout::Dense,
        &mut || false,
        &mut dense,
    )
    .unwrap();
    let (header, live, write_version) = &dense.headers[&RecordSlot(0)];
    assert_eq!(header.id.0.as_ref(), "retained");
    assert!(*live);
    assert_eq!(*write_version, Some(version));
    assert_eq!(dense.bodies[&RecordSlot(0)].value, Value::String(text));
    session.finish().unwrap();
}
