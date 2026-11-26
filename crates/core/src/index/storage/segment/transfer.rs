//! Checked transfer of record metadata and retained bodies.

use std::io::{self, Read, Write};
use std::num::NonZeroUsize;

use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::transfer::{BatchLimit, StorageError, StorageResult};
use crate::inventory::{ActiveRecords, FileEntry};
use crate::model::RecordSlot;
use crate::records::RecordHeader;

use super::maintenance::SegmentPartition;
use super::{
    RecordBacking, RecordOrigin, RecordWriteVersion, SegmentReader, visit_record_headers,
    visit_segment_files,
};

const RECORD_LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(128).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);
pub(crate) const BODY_PART_BYTES: usize = 64 * 1024;
pub(crate) const RETAINED_BODY_TRANSFER_VERSION: u32 = 1;

/// A sequential stream of the internal retained encoding. A successful
/// zero-length read is EOF. The buffer bounds every backend read.
pub(crate) trait RetainedBodyBytes {
    fn read_part(&mut self, buffer: &mut [u8]) -> StorageResult<usize>;
}

pub(crate) fn read_retained_body(
    source: &dyn SegmentReader,
    slot: RecordSlot,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<crate::records::RetainedBody> {
    const OP: &str = "read retained body for transfer";
    if cancelled() {
        return Err(StorageError::Cancelled);
    }
    let Some(parts) = source.retained_body_bytes(slot)? else {
        return source
            .retained_body(slot)?
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| StorageError::invalid(OP, "retained body missing"));
    };
    struct Decoder<'a> {
        parts: Box<dyn RetainedBodyBytes + 'a>,
        cancelled: &'a dyn Fn() -> bool,
        error: Option<StorageError>,
    }
    impl Read for Decoder<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let result = if (self.cancelled)() {
                Err(StorageError::Cancelled)
            } else {
                read_body_part(self.parts.as_mut(), buffer)
            };
            result.map_err(|error| {
                self.error = Some(error);
                io::Error::other("read retained transfer part failed")
            })
        }
    }
    let mut decoder = Decoder {
        parts,
        cancelled,
        error: None,
    };
    let (value, text) = bincode::deserialize_from(&mut decoder).map_err(|error| {
        decoder
            .error
            .take()
            .unwrap_or_else(|| StorageError::invalid(OP, error.to_string()))
    })?;
    if decoder.read(&mut [0]).map_err(|error| {
        decoder
            .error
            .take()
            .unwrap_or_else(|| StorageError::invalid(OP, error.to_string()))
    })? != 0
    {
        return Err(StorageError::invalid(
            OP,
            "trailing bytes after retained body",
        ));
    }
    Ok(crate::records::RetainedBody { value, text })
}

fn read_body_part(parts: &mut dyn RetainedBodyBytes, buffer: &mut [u8]) -> StorageResult<usize> {
    let limit = buffer.len().min(BODY_PART_BYTES);
    if limit == 0 {
        return Ok(0);
    }
    let bytes = parts.read_part(&mut buffer[..limit])?;
    if bytes > limit {
        return Err(StorageError::invalid(
            "read retained transfer part",
            "part exceeds buffer",
        ));
    }
    Ok(bytes)
}

/// Identity transfer keeps retired slots and their liveness. Dense transfer
/// writes only the selected slots, in their original order.
#[derive(Clone, Copy)]
pub(crate) enum RecordLayout {
    Identity,
    Dense,
}

/// The records and file associations to emit for a file-backed import.
#[derive(Clone, Copy)]
pub(crate) enum FileSegmentLayout<'a> {
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "identity imports are exercised by backend conformance"
        )
    )]
    Identity(&'a ActiveRecords),
    Dense {
        partition: &'a SegmentPartition,
        catalog: &'a dyn FileCatalogReader,
    },
}

impl FileSegmentLayout<'_> {
    pub(crate) fn visit_records(
        self,
        source: &dyn SegmentReader,
        cancelled: &mut dyn FnMut() -> bool,
        visit: &mut dyn FnMut(&[RecordTransferRow<'_>]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        let (active, layout) = match self {
            Self::Identity(active) => (active, RecordLayout::Identity),
            Self::Dense { partition, .. } => (&partition.slots, RecordLayout::Dense),
        };
        visit_record_rows(source, active, layout, cancelled, visit)
    }

    pub(crate) fn visit_files(
        self,
        source: &dyn SegmentReader,
        cancelled: &mut dyn FnMut() -> bool,
        mut visit: impl FnMut(&FileEntry) -> StorageResult<()>,
    ) -> StorageResult<()> {
        let mut count = 0;
        visit_segment_files(source, |file| {
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            match self {
                Self::Identity(_) => visit(file)?,
                Self::Dense { partition, catalog } => {
                    if !partition.files.contains(&file.id) {
                        return Ok(());
                    }
                    let current = catalog.file(file.id)?.ok_or_else(|| {
                        StorageError::invalid("transfer file segment", "missing selected file")
                    })?;
                    if file.revision != current.revision || file.created_ns != current.created_ns {
                        return Err(StorageError::invalid(
                            "transfer file segment",
                            "selected file metadata changed",
                        ));
                    }
                    let mut file = file.clone();
                    file.relative_path.clone_from(&current.relative_path);
                    visit(&file)?;
                }
            }
            count += 1;
            Ok(())
        })?;
        let expected = match self {
            Self::Identity(_) => source.file_count(),
            Self::Dense { partition, .. } => partition.files.len(),
        };
        if count != expected {
            return Err(StorageError::invalid(
                "transfer file segment",
                "file selection has incomplete coverage",
            ));
        }
        Ok(())
    }
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
            let live = active.contains(header.slot)?;
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
        if let Some(mut parts) = source.retained_body_bytes(row.header.slot)? {
            let mut buffer = vec![0; BODY_PART_BYTES];
            loop {
                if cancelled() {
                    return Err(StorageError::Cancelled);
                }
                let bytes = read_body_part(parts.as_mut(), &mut buffer)?;
                sink.retained_part(
                    row.destination,
                    RETAINED_BODY_TRANSFER_VERSION,
                    &buffer[..bytes],
                    bytes == 0,
                )?;
                if bytes == 0 {
                    break;
                }
            }
            continue;
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
