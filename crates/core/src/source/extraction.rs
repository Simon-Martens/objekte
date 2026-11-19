//! Source file extraction, work accounting, and outcome delivery.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::inventory::{ByteRange, SourceRevision};

use super::access::check_revision;
use super::framing::validate_reader_ranges;
use super::{
    DiagnosticCategory, ExtractedRecord, ReaderDiagnostic, ReaderFailure, SourceFormat,
    SourceOutcome, SourceReader, registry::SourceRegistry,
};

/// Framing and parse counts from one source extraction.
#[derive(Default, Clone, Copy, Debug)]
pub struct ExtractionCounters {
    pub framed_records: u64,
    pub record_parses: u64,
    pub failed_parses: u64,
}

/// Sampled source-stage wall time for one fresh-build file. Extraction includes
/// reader parsing and the caller's record-consumption callback.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceBuildSample {
    pub open_ns: u64,
    pub read_ns: u64,
    pub hash_ns: u64,
    pub frame_ns: u64,
    pub extraction_ns: u64,
    pub bytes_read: u64,
    pub read_calls: u64,
}

/// Opt-in, batch-local observations for reusable normal-file input storage.
/// A growth file is one whose final capacity exceeds its entry capacity;
/// multiple allocator calls within that file are not separately counted.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceBufferProfile {
    pub files_read: u64,
    pub bytes_read: u64,
    pub growth_files: u64,
    pub peak_capacity_bytes: u64,
}

/// Large ordinary files are still processed, but their buffer is not retained
/// for the many small files that may follow in the same batch.
pub(crate) const MAX_RETAINED_BUILD_BUFFER_BYTES: usize = 1024 * 1024;

pub(crate) fn trim_oversized_build_buffer(buffer: &mut Vec<u8>) -> bool {
    if buffer.capacity() <= MAX_RETAINED_BUILD_BUFFER_BYTES {
        return false;
    }
    *buffer = Vec::new();
    true
}

struct TimedBuildReader<'a> {
    inner: &'a mut std::fs::File,
    sample: &'a mut SourceBuildSample,
}

impl Read for TimedBuildReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let started = crate::measurement::timer::DetailedInstant::now();
        let count = self.inner.read(bytes)?;
        self.sample.read_ns += started.elapsed().as_nanos() as u64;
        self.sample.read_calls += 1;
        self.sample.bytes_read += count as u64;
        Ok(count)
    }
}

/// Call `consume` once for every independently parseable record. No index
/// family rereads or reparses the source; builders share the extracted value.
pub fn extract_file<F>(
    path: &Path,
    format: SourceFormat,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(ByteRange, ExtractedRecord) -> Result<(), String>,
{
    let source = SourceRegistry::builtin().resolve(&format)?;
    extract_file_with_reader(path, source.reader(), expected, consume)
}

pub fn extract_file_with_reader<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(ByteRange, ExtractedRecord) -> Result<(), String>,
{
    extract_file_outcomes(path, source, expected, |outcome| match outcome {
        SourceOutcome::Logical { range, record } => consume(range, record),
        SourceOutcome::Malformed {
            range, diagnostic, ..
        } => Err(format!(
            "{}:{}..{}: {}",
            path.display(),
            range.start,
            range.end,
            diagnostic.message
        )),
        SourceOutcome::Unreadable { diagnostic } => {
            Err(format!("{}: {}", path.display(), diagnostic.message))
        }
    })
}

pub(crate) fn frame_file_for_build(
    path: &Path,
    source: &dyn SourceReader,
) -> Result<Option<(Arc<Vec<u8>>, Vec<ByteRange>)>, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Ok(None);
    }
    let ranges = match source.frame_outcome(&bytes) {
        Ok(ranges) => ranges,
        Err(ReaderFailure::Malformed(_)) => return Ok(None),
        Err(ReaderFailure::Fatal(error)) => {
            return Err(format!("{}: reader fatal: {error}", path.display()));
        }
    };
    validate_reader_ranges(&ranges, bytes.len())?;
    Ok(Some((Arc::new(bytes), ranges)))
}

/// One bounded piece of a newline-framed source. Ranges address `bytes`;
/// `file_offset` translates them into the repository's absolute file ranges.
pub(crate) fn extract_framed_build_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    bytes: &[u8],
    ranges: &[ByteRange],
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_framed_outcomes_impl(path, source, bytes, ranges, true, consume)
}

pub(crate) fn extract_framed_outcomes_impl<F>(
    path: &Path,
    source: &dyn SourceReader,
    bytes: &[u8],
    ranges: &[ByteRange],
    for_build: bool,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let mut counters = ExtractionCounters::default();
    let mut coordinate_cursor = FileCoordinateCursor::default();
    for &range in ranges {
        let part = &bytes[range.start as usize..range.end as usize];
        counters.framed_records += 1;
        match if for_build {
            source.parse_for_build_outcome(part)
        } else {
            source.parse_outcome(part)
        } {
            Ok(record) => {
                counters.record_parses += 1;
                consume(SourceOutcome::Logical { range, record })?;
            }
            Err(ReaderFailure::Malformed(mut diagnostic)) => {
                counters.failed_parses += 1;
                file_coordinates(bytes, range, &mut diagnostic, &mut coordinate_cursor);
                consume(SourceOutcome::Malformed {
                    range,
                    diagnostic,
                    bytes: part.to_vec(),
                })?;
            }
            Err(ReaderFailure::Fatal(error)) => {
                return Err(format!(
                    "{}:{}..{}: reader fatal: {error}",
                    path.display(),
                    range.start,
                    range.end
                ));
            }
        }
    }
    Ok(counters)
}

#[cfg(test)]
thread_local! {
    pub(crate) static FILE_COORDINATE_SCANNED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Running file position for ordered reader ranges, bounded independently of file size.
#[derive(Default)]
pub(crate) struct FileCoordinateCursor {
    scanned_to: usize,
    earlier_lines: u64,
    line_start: usize,
}

pub(crate) fn file_coordinates(
    file: &[u8],
    range: ByteRange,
    diagnostic: &mut ReaderDiagnostic,
    cursor: &mut FileCoordinateCursor,
) {
    let start = range.start as usize;
    // Reader ranges are validated as ordered and non-overlapping before this
    // loop. Scan the intervening bytes once, including gaps between records.
    let prefix = &file[cursor.scanned_to..start];
    #[cfg(test)]
    FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.set(scanned.get() + prefix.len()));
    for (index, byte) in prefix.iter().enumerate() {
        if *byte == b'\n' {
            cursor.earlier_lines += 1;
            cursor.line_start = cursor.scanned_to + index + 1;
        }
    }
    cursor.scanned_to = start;
    if let Some(line) = diagnostic.line {
        diagnostic.line = line.checked_add(cursor.earlier_lines);
        if line == 1 {
            diagnostic.column = diagnostic
                .column
                .and_then(|column| column.checked_add((start - cursor.line_start) as u64));
        }
    }
}

pub fn extract_file_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_file_outcomes_impl(
        path, source, expected, false, true, None, None, None, None, consume,
    )
}

/// Search a source without testing whether the saved discovery metadata is
/// still current. An index lifecycle operation owns that synchronization.
pub(crate) fn extract_file_outcomes_for_search<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_file_outcomes_impl(
        path, source, expected, false, false, None, None, None, None, consume,
    )
}

pub(crate) fn extract_file_build_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_file_outcomes_impl(
        path, source, expected, true, false, None, None, None, None, consume,
    )
}

/// Capture the exact whole-file digest while the fresh builder's normal
/// extraction read is still resident. An unreadable file has no fingerprint.
pub(crate) fn extract_file_build_outcomes_hashed_with_buffer<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    buffer: &mut Vec<u8>,
    profile: Option<&mut SourceBuildSample>,
    buffer_profile: Option<&mut SourceBufferProfile>,
    consume: F,
) -> Result<(ExtractionCounters, Option<[u8; 32]>), String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let mut fingerprint = None;
    let counters = extract_file_outcomes_impl(
        path,
        source,
        expected,
        true,
        false,
        Some(&mut fingerprint),
        profile,
        Some(buffer),
        buffer_profile,
        consume,
    )?;
    Ok((counters, fingerprint))
}

fn extract_file_outcomes_impl<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    for_build: bool,
    check_freshness: bool,
    fingerprint: Option<&mut Option<[u8; 32]>>,
    mut profile: Option<&mut SourceBuildSample>,
    buffer: Option<&mut Vec<u8>>,
    mut buffer_profile: Option<&mut SourceBufferProfile>,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let open_started = crate::measurement::timer::start_if_enabled(profile.is_some());
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            consume(SourceOutcome::Unreadable {
                diagnostic: ReaderDiagnostic::new(
                    DiagnosticCategory::Io,
                    "source.open",
                    error.to_string(),
                ),
            })?;
            return Ok(ExtractionCounters::default());
        }
    };
    if let (Some(started), Some(sample)) = (open_started, profile.as_deref_mut()) {
        sample.open_ns = started.elapsed().as_nanos() as u64;
    }
    if check_freshness {
        check_revision(path, &file, expected)?;
    }
    let reusable = buffer.is_some();
    let mut owned_bytes = Vec::new();
    let bytes = buffer.unwrap_or(&mut owned_bytes);
    bytes.clear();
    #[cfg(feature = "detailed-profiling")]
    let previous_capacity = bytes.capacity();
    if reusable {
        let hint = usize::try_from(expected.size)
            .unwrap_or(usize::MAX)
            .min(MAX_RETAINED_BUILD_BUFFER_BYTES);
        if bytes.capacity() < hint {
            bytes.reserve(hint);
        }
    }
    let read_result = if let Some(sample) = profile.as_deref_mut() {
        TimedBuildReader {
            inner: &mut file,
            sample,
        }
        .read_to_end(&mut *bytes)
    } else {
        file.read_to_end(&mut *bytes)
    };
    if let Some(stats) = buffer_profile.as_deref_mut() {
        stats.files_read += 1;
        stats.bytes_read = stats.bytes_read.saturating_add(bytes.len() as u64);
        #[cfg(feature = "detailed-profiling")]
        {
            stats.growth_files += u64::from(bytes.capacity() > previous_capacity);
            stats.peak_capacity_bytes = stats.peak_capacity_bytes.max(bytes.capacity() as u64);
        }
    }
    if let Err(error) = read_result {
        consume(SourceOutcome::Unreadable {
            diagnostic: ReaderDiagnostic::new(
                DiagnosticCategory::Io,
                "source.read",
                error.to_string(),
            ),
        })?;
        return Ok(ExtractionCounters::default());
    }
    let hash_started = crate::measurement::timer::start_if_enabled(profile.is_some());
    if let Some(fingerprint) = fingerprint {
        if bytes.len() as u64 == expected.size {
            *fingerprint = Some(Sha256::digest(bytes.as_slice()).into());
        }
    }
    if let (Some(started), Some(sample)) = (hash_started, profile.as_deref_mut()) {
        sample.hash_ns = started.elapsed().as_nanos() as u64;
    }
    if check_freshness {
        check_revision(path, &file, expected)?;
        if bytes.len() as u64 != expected.size {
            return Err("stale source revision during extraction".into());
        }
    }
    let frame_started = crate::measurement::timer::start_if_enabled(profile.is_some());
    let ranges = match source.frame_outcome(&bytes) {
        Ok(ranges) => ranges,
        Err(ReaderFailure::Malformed(mut diagnostic)) => {
            let size = u64::try_from(bytes.len()).map_err(|_| "source size overflow")?;
            let range = ByteRange::checked(0, size, size)?;
            file_coordinates(
                &bytes,
                range,
                &mut diagnostic,
                &mut FileCoordinateCursor::default(),
            );
            consume(SourceOutcome::Malformed {
                range,
                diagnostic,
                bytes: std::mem::take(bytes),
            })?;
            if check_freshness {
                check_revision(path, &file, expected)?;
            }
            return Ok(ExtractionCounters {
                framed_records: 1,
                record_parses: 0,
                failed_parses: 0,
            });
        }
        Err(ReaderFailure::Fatal(error)) => {
            return Err(format!("{}: reader fatal: {error}", path.display()));
        }
    };
    if let (Some(started), Some(sample)) = (frame_started, profile.as_deref_mut()) {
        sample.frame_ns = started.elapsed().as_nanos() as u64;
    }
    validate_reader_ranges(&ranges, bytes.len())?;
    let extraction_started = crate::measurement::timer::start_if_enabled(profile.is_some());
    let counters =
        extract_framed_outcomes_impl(path, source, &bytes, &ranges, for_build, &mut consume)?;
    if let (Some(started), Some(sample)) = (extraction_started, profile.as_deref_mut()) {
        sample.extraction_ns = started.elapsed().as_nanos() as u64;
    }
    if check_freshness {
        check_revision(path, &file, expected)?;
    }
    Ok(counters)
}

#[cfg(test)]
mod tests;
