//! Checked transfer of record metadata and retained bodies.

use std::io::{self, Write};
use std::num::NonZeroUsize;

use crate::index::storage::transfer::{BatchLimit, StorageError, StorageResult};
use crate::inventory::ActiveRecords;
use crate::model::RecordSlot;
use crate::records::RecordHeader;

use super::{RecordBacking, RecordOrigin, RecordWriteVersion, SegmentReader, visit_record_headers};

const RECORD_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(128).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);
const BODY_PART_BYTES: usize = 64 * 1024;
const RETAINED_BODY_TRANSFER_VERSION: u32 = 1;

/// Identity transfer keeps retired slots and their liveness. Dense transfer
/// writes only the selected slots, in their original order.
#[derive(Clone, Copy)]
pub(crate) enum RecordLayout {
    Identity,
    Dense,
}

pub(crate) struct RecordTransferRow<'a> {
    pub header: &'a RecordHeader,
    pub destination: RecordSlot,
    pub origin: RecordOrigin,
    pub write_version: Option<RecordWriteVersion>,
    pub live: bool,
}

/// Drain each metadata batch synchronously. Body parts follow the metadata
/// for their slot and use the internal retained-value transfer encoding.
/// A final part may be empty when the preceding part ended on a boundary.
pub(crate) trait RecordTransferSink {
    fn records(&mut self, records: &[RecordTransferRow<'_>]) -> StorageResult<()>;

    fn wants_retained_bodies(&self) -> bool;

    fn retained_part(
        &mut self,
        slot: RecordSlot,
        version: u32,
        part: &[u8],
        last: bool,
    ) -> StorageResult<()>;
}

/// Use bounded record rows for planning. Retained payloads remain unopened.
pub(crate) fn visit_record_rows(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    layout: RecordLayout,
    cancelled: &mut dyn FnMut() -> bool,
    visit: &mut dyn FnMut(&[RecordTransferRow<'_>]) -> StorageResult<()>,
) -> StorageResult<()> {
    struct MetadataOnly<'a>(&'a mut dyn FnMut(&[RecordTransferRow<'_>]) -> StorageResult<()>);
    impl RecordTransferSink for MetadataOnly<'_> {
        fn records(&mut self, rows: &[RecordTransferRow<'_>]) -> StorageResult<()> {
            (self.0)(rows)
        }
        fn wants_retained_bodies(&self) -> bool {
            false
        }
        fn retained_part(&mut self, _: RecordSlot, _: u32, _: &[u8], _: bool) -> StorageResult<()> {
            Err(StorageError::invalid(
                "transfer retained body",
                "sink declined retained bodies",
            ))
        }
    }
    transfer_records(source, active, layout, cancelled, &mut MetadataOnly(visit))
}

/// Transfer logical record headers and origins without contacting a source
/// file or a versioned reader. Sealed retained bodies may be borrowed and
/// emitted in small blocks. The selection refers to the source's local slots.
pub(crate) fn transfer_records(
    source: &dyn SegmentReader,
    active: &ActiveRecords,
    layout: RecordLayout,
    cancelled: &mut dyn FnMut() -> bool,
    sink: &mut dyn RecordTransferSink,
) -> StorageResult<()> {
    const OP: &str = "transfer segment records";
    if active.slot_count() as usize != source.descriptor().slots {
        return Err(StorageError::invalid(
            OP,
            "membership covers a different segment domain",
        ));
    }
    if cancelled() {
        return Err(StorageError::Cancelled);
    }
    let mut emitted = 0u32;
    let backing = source.backing_kind();
    visit_record_headers(source, |headers| {
        let mut rows = Vec::new();
        let mut bytes = 0;
        for header in headers {
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            let live = active.contains(header.slot);
            if matches!(layout, RecordLayout::Dense) && !live {
                continue;
            }
            let destination = match layout {
                RecordLayout::Identity => header.slot,
                RecordLayout::Dense => RecordSlot(emitted),
            };
            let origin = source
                .origin(header.slot)?
                .ok_or_else(|| StorageError::invalid(OP, "record origin missing"))?;
            let kind = match origin {
                RecordOrigin::FileRange(locator) if locator.range.is_some() => {
                    RecordBacking::FileRange
                }
                RecordOrigin::FileRange(_) => {
                    return Err(StorageError::invalid(OP, "file origin has no range"));
                }
                RecordOrigin::Retained(_) => RecordBacking::Retained,
                RecordOrigin::Provider { .. } => RecordBacking::Provider,
            };
            if kind != backing {
                return Err(StorageError::invalid(
                    OP,
                    "record origin and backing disagree",
                ));
            }
            let write_version = source.write_version(header.slot)?;
            let row_bytes = size_of::<RecordTransferRow<'_>>()
                .saturating_add(size_of::<RecordHeader>())
                .saturating_add(header.id.0.len());
            if !RECORD_LIMIT.accepts(OP, rows.len(), bytes, row_bytes)? {
                transfer_record_batch(source, &rows, cancelled, sink)?;
                rows.clear();
                bytes = 0;
                RECORD_LIMIT.accepts(OP, 0, 0, row_bytes)?;
            }
            rows.push(RecordTransferRow {
                header,
                destination,
                origin,
                write_version,
                live,
            });
            bytes += row_bytes;
            emitted = emitted
                .checked_add(1)
                .ok_or_else(|| StorageError::invalid(OP, "record count overflow"))?;
        }
        transfer_record_batch(source, &rows, cancelled, sink)
    })?;
    let expected = match layout {
        RecordLayout::Identity => u64::from(active.slot_count()),
        RecordLayout::Dense => active.len(),
    };
    if u64::from(emitted) != expected {
        return Err(StorageError::invalid(
            OP,
            "transfer did not cover its record selection",
        ));
    }
    Ok(())
}

fn transfer_record_batch(
    source: &dyn SegmentReader,
    rows: &[RecordTransferRow<'_>],
    cancelled: &mut dyn FnMut() -> bool,
    sink: &mut dyn RecordTransferSink,
) -> StorageResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    if rows
        .iter()
        .any(|row| !matches!(row.origin, RecordOrigin::FileRange(_)) && row.write_version.is_none())
    {
        return Err(StorageError::invalid(
            "transfer segment records",
            "direct write version missing",
        ));
    }
    sink.records(rows)?;
    if !sink.wants_retained_bodies() {
        return Ok(());
    }
    for row in rows {
        if !matches!(row.origin, RecordOrigin::Retained(_)) {
            continue;
        }
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let body = source.retained_body(row.header.slot)?.ok_or_else(|| {
            StorageError::invalid(
                "transfer retained body",
                "body not available for retained slot",
            )
        })?;
        let mut out = BodyParts {
            target: row.destination,
            sink,
            cancelled,
            pending: Vec::with_capacity(BODY_PART_BYTES),
            error: None,
        };
        if let Err(error) = bincode::serialize_into(&mut out, &(&body.value, &body.text)) {
            return Err(out.error.unwrap_or_else(|| {
                StorageError::invalid("encode retained body", error.to_string())
            }));
        }
        out.emit(true)?;
    }
    Ok(())
}

struct BodyParts<'a> {
    target: RecordSlot,
    sink: &'a mut dyn RecordTransferSink,
    cancelled: &'a mut dyn FnMut() -> bool,
    pending: Vec<u8>,
    error: Option<StorageError>,
}

impl BodyParts<'_> {
    fn emit(&mut self, last: bool) -> StorageResult<()> {
        if (self.cancelled)() {
            return Err(StorageError::Cancelled);
        }
        self.sink.retained_part(
            self.target,
            RETAINED_BODY_TRANSFER_VERSION,
            &self.pending,
            last,
        )?;
        self.pending.clear();
        Ok(())
    }
}

impl Write for BodyParts<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.pending.len() == BODY_PART_BYTES
            && let Err(error) = self.emit(false)
        {
            self.error = Some(error);
            return Err(io::Error::other("retained body transfer failed"));
        }
        let count = bytes.len().min(BODY_PART_BYTES - self.pending.len());
        self.pending.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
