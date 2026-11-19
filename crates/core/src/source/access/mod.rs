//! Query-scoped, bounded source handles and range windows.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::inventory::{ByteRange, SourceRevision};
use crate::measurement::SourceUnavailableKind;
use crate::streaming::{ExecutionContext, ExecutionErrorKind};

pub(crate) mod session;

pub(crate) use session::SourceAccessSession;

#[cfg(test)]
thread_local! {
    static SEARCH_READ_OBSERVATIONS:
        std::cell::RefCell<Vec<(std::path::PathBuf, u64, u64)>> =
            const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn take_search_read_observations() -> Vec<(std::path::PathBuf, u64, u64)> {
    SEARCH_READ_OBSERVATIONS.with(|observations| std::mem::take(&mut *observations.borrow_mut()))
}

#[cfg(test)]
fn record_search_read(path: &Path, range: ByteRange) {
    SEARCH_READ_OBSERVATIONS.with(|observations| {
        observations
            .borrow_mut()
            .push((path.to_path_buf(), range.start, range.end));
    });
}

fn source_changed(execution: Option<&ExecutionContext>, message: String) -> String {
    if let Some(execution) = execution {
        execution.fail(ExecutionErrorKind::SourceChanged, message)
    } else {
        message
    }
}

/// Diagnose an unavailable search source without comparing it with the
/// index's saved file metadata or content.
pub(crate) fn probe_source_unavailable_for_search(
    path: &Path,
    revision: SourceRevision,
    range: Option<ByteRange>,
) -> Option<SourceUnavailableKind> {
    match std::fs::File::open(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(SourceUnavailableKind::Missing);
        }
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
        Ok(_) => {}
    }
    if range.is_some_and(|range| range.start > range.end || range.end > revision.size) {
        return Some(SourceUnavailableKind::UnusableRange);
    }
    None
}

pub(crate) fn classify_source_read_failure_for_search(
    path: &Path,
    revision: SourceRevision,
    range: Option<ByteRange>,
) -> SourceUnavailableKind {
    probe_source_unavailable_for_search(path, revision, range)
        .unwrap_or(SourceUnavailableKind::Unreadable)
}

pub(crate) fn check_revision(
    path: &Path,
    file: &std::fs::File,
    expected: SourceRevision,
) -> Result<(), String> {
    let opened = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    let named = SourceRevision::from_metadata(
        &std::fs::metadata(path).map_err(|e| format!("stale source revision: {e}"))?,
    )?;
    if opened != expected || named != expected {
        return Err("stale source revision".into());
    }
    Ok(())
}

fn opened_revision_matches(file: &std::fs::File, expected: SourceRevision) -> Result<bool, String> {
    let opened = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    Ok(opened == expected)
}

#[derive(Clone, Copy)]
struct SourceIdentityProof {
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
}

fn opened_identity_matches(
    file: &mut std::fs::File,
    expected: SourceRevision,
    proof: SourceIdentityProof,
) -> Result<bool, String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if SourceRevision::from_metadata(&metadata)? != expected {
        return Ok(false);
    }
    if let Some(created_ns) = proof.created_ns {
        let created = metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as i128);
        return Ok(created == Some(created_ns));
    }
    let Some(fingerprint) = proof.fingerprint else {
        return Ok(false);
    };
    // Creation time is unavailable on this filesystem. Hash the same opened
    // handle, restoring its cursor so a subsequent range read stays correct.
    let position = file.stream_position().map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let digest = (|| -> Result<[u8; 32], String> {
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(hasher.finalize().into())
    })();
    file.seek(SeekFrom::Start(position))
        .map_err(|error| error.to_string())?;
    Ok(digest? == fingerprint && opened_revision_matches(file, expected)?)
}

fn check_opened_revision(file: &std::fs::File, expected: SourceRevision) -> Result<(), String> {
    if !opened_revision_matches(file, expected)? {
        return Err("stale source revision".into());
    }
    Ok(())
}

/// Read only a stored range after checking the recorded source revision.
pub fn read_record_range(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    check_opened_revision(&file, revision)?;
    ByteRange::checked(range.start, range.end, revision.size)?;
    read_record_range_from_opened(&mut file, range)
}

/// Query read of a range published by an index. The index lifecycle owns
/// source validation; search only checks the stored range and performs I/O.
pub(crate) fn read_record_range_for_search(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
    execution: Option<&ExecutionContext>,
) -> Result<Vec<u8>, String> {
    #[cfg(test)]
    record_search_read(path, range);
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    ByteRange::checked(range.start, range.end, revision.size)?;
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    read_record_range_from_opened(&mut file, range)
}

/// Lifecycle read tied to a pinned file incarnation. When birth time is not
/// available, the raw digest proves the opened handle instead.
#[cfg(test)]
pub(crate) fn read_record_range_checked(
    path: &Path,
    revision: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    read_record_range_checked_impl(path, revision, created_ns, fingerprint, range, None)
}

#[cfg(test)]
fn read_record_range_checked_impl(
    path: &Path,
    revision: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: ByteRange,
    execution: Option<&ExecutionContext>,
) -> Result<Vec<u8>, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let proof = SourceIdentityProof {
        created_ns,
        fingerprint,
    };
    if !opened_identity_matches(&mut file, revision, proof)? {
        return Err(source_changed(execution, "stale source identity".into()));
    }
    ByteRange::checked(range.start, range.end, revision.size)?;
    let bytes = read_record_range_from_opened(&mut file, range)?;
    if !opened_identity_matches(&mut file, revision, proof)? {
        return Err(source_changed(execution, "stale source identity".into()));
    }
    Ok(bytes)
}

/// Query read with an explicit typed source-change channel.
#[cfg(test)]
pub(crate) fn read_record_range_with_execution(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
    execution: &ExecutionContext,
) -> Result<Vec<u8>, String> {
    execution.check_string()?;
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !opened_revision_matches(&file, revision)? {
        return Err(source_changed(
            Some(execution),
            "stale source revision".into(),
        ));
    }
    ByteRange::checked(range.start, range.end, revision.size)?;
    read_record_range_from_opened(&mut file, range)
}

/// Build-only fallback read: discovery supplied the source revision, while
/// construction assumes the source remains stable and performs no new probe.
pub(crate) fn read_record_range_for_build(
    path: &Path,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ByteRange::checked(range.start, range.end, u64::MAX)?;
    read_record_range_from_opened(&mut file, range)
}

fn read_record_range_from_opened(
    file: &mut std::fs::File,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let len = usize::try_from(range.len()).map_err(|_| "record range too large")?;
    let mut bytes = vec![0; len];
    file.seek(SeekFrom::Start(range.start))
        .map_err(|e| e.to_string())?;
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Read a bounded batch from one file handle. Nearby ranges share a window;
/// distant ranges remain sparse reads. The returned byte count is physical
/// source bytes, including small gaps inside coalesced windows.
pub fn read_record_ranges(
    path: &Path,
    revision: SourceRevision,
    ranges: &[(u32, ByteRange)],
) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
    RecordRangeReader::open(path, revision)?.read(ranges)
}

/// One opened source handle for a bounded sequence of range batches.
pub struct RecordRangeReader {
    file: std::fs::File,
    revision: SourceRevision,
    check_freshness: bool,
    first_read: bool,
    position: Option<u64>,
    execution: Option<ExecutionContext>,
    identity: Option<SourceIdentityProof>,
    #[cfg(test)]
    pub(crate) seeks: u64,
    #[cfg(test)]
    pub(crate) max_window_read: usize,
}

impl RecordRangeReader {
    pub fn open(path: &Path, revision: SourceRevision) -> Result<Self, String> {
        Self::open_with_execution(path, revision, None)
    }

    pub(crate) fn open_with_execution(
        path: &Path,
        revision: SourceRevision,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        Self::open_with_identity(path, revision, None, execution)
    }

    /// Open a published source for search without checking its current
    /// metadata or content against the index's recorded revision.
    pub(crate) fn open_for_search(
        path: &Path,
        revision: SourceRevision,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        let file =
            std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            file,
            revision,
            check_freshness: false,
            first_read: true,
            position: Some(0),
            execution: execution.cloned(),
            identity: None,
            #[cfg(test)]
            seeks: 0,
            #[cfg(test)]
            max_window_read: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_checked(
        path: &Path,
        revision: SourceRevision,
        created_ns: Option<i128>,
        fingerprint: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        Self::open_with_identity(
            path,
            revision,
            Some(SourceIdentityProof {
                created_ns,
                fingerprint,
            }),
            None,
        )
    }

    fn open_with_identity(
        path: &Path,
        revision: SourceRevision,
        identity: Option<SourceIdentityProof>,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let matched = if let Some(proof) = identity {
            opened_identity_matches(&mut file, revision, proof)?
        } else {
            opened_revision_matches(&file, revision)?
        };
        if !matched {
            let message = if identity.is_some() {
                "stale source identity"
            } else {
                "stale source revision"
            };
            return Err(source_changed(execution, message.into()));
        }
        Ok(Self {
            file,
            revision,
            check_freshness: true,
            first_read: true,
            position: Some(0),
            execution: execution.cloned(),
            identity,
            #[cfg(test)]
            seeks: 0,
            #[cfg(test)]
            max_window_read: 0,
        })
    }

    fn current_matches(&mut self) -> Result<bool, String> {
        if let Some(proof) = self.identity {
            opened_identity_matches(&mut self.file, self.revision, proof)
        } else {
            opened_revision_matches(&self.file, self.revision)
        }
    }

    fn check_current(&mut self) -> Result<(), String> {
        if !self.current_matches()? {
            let message = if self.identity.is_some() {
                "stale source identity"
            } else {
                "stale source revision"
            };
            return Err(source_changed(self.execution.as_ref(), message.into()));
        }
        Ok(())
    }

    fn read_at(&mut self, start: u64, bytes: &mut [u8]) -> Result<(), String> {
        if self.position != Some(start) {
            // A failed seek or a partial read can leave a reused handle anywhere.
            self.position = None;
            #[cfg(test)]
            {
                self.seeks += 1;
            }
            self.file
                .seek(SeekFrom::Start(start))
                .map_err(|e| e.to_string())?;
            self.position = Some(start);
        }
        if let Err(error) = self.file.read_exact(bytes) {
            self.position = None;
            return Err(error.to_string());
        }
        self.position = Some(start + bytes.len() as u64);
        Ok(())
    }

    pub fn read(
        &mut self,
        ranges: &[(u32, ByteRange)],
    ) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
        if self.first_read {
            self.first_read = false;
        } else if self.check_freshness {
            self.check_current()?;
        }
        if let [(slot, range)] = ranges {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
            let len = usize::try_from(range.len()).map_err(|_| "record range too large")?;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut bytes = vec![0; len];
            self.read_at(range.start, &mut bytes)?;
            if self.check_freshness && self.identity.is_some() {
                self.check_current()?;
            }
            return Ok((vec![(*slot, bytes)], len as u64));
        }
        let mut ordered = ranges.to_vec();
        ordered.sort_by_key(|(_, range)| range.start);
        for (_, range) in &ordered {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
        }
        let mut output = Vec::with_capacity(ordered.len());
        let mut bytes_read = 0u64;
        let mut cursor = 0;
        while cursor < ordered.len() {
            let start = ordered[cursor].1.start;
            let mut end = ordered[cursor].1.end;
            let mut next = cursor + 1;
            while next < ordered.len() {
                let candidate = ordered[next].1;
                if candidate.start > end.saturating_add(4096)
                    || candidate.end.saturating_sub(start) > 1024 * 1024
                {
                    break;
                }
                end = end.max(candidate.end);
                next += 1;
            }
            let len = usize::try_from(end - start).map_err(|_| "read window too large")?;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut buffer = vec![0; len];
            self.read_at(start, &mut buffer)?;
            bytes_read += len as u64;
            for (slot, range) in &ordered[cursor..next] {
                let a = (range.start - start) as usize;
                let b = (range.end - start) as usize;
                output.push((*slot, buffer[a..b].to_vec()));
            }
            cursor = next;
        }
        if self.check_freshness && self.identity.is_some() {
            self.check_current()?;
        }
        Ok((output, bytes_read))
    }

    /// Stream-specific direct range reads. The checked sum bounds all returned
    /// byte vectors; no gap window or second copy of any record is allocated.
    pub(crate) fn read_bounded(
        &mut self,
        ranges: &[(u32, ByteRange)],
        max_total_bytes: usize,
    ) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
        if self.first_read {
            self.first_read = false;
        } else if self.check_freshness {
            self.check_current()?;
        }
        let mut ordered = ranges.to_vec();
        ordered.sort_by_key(|(_, range)| range.start);
        let mut total = 0usize;
        for (_, range) in &ordered {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
            let len = usize::try_from(range.len())
                .map_err(|_| "stream resource: record range too large")?;
            total = total
                .checked_add(len)
                .ok_or("stream resource: source range budget overflow")?;
            if total > max_total_bytes {
                return Err("stream resource: source ranges exceed read allowance".into());
            }
        }
        let mut output = Vec::with_capacity(ordered.len());
        for (slot, range) in ordered {
            let len = range.len() as usize;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut bytes = vec![0; len];
            self.read_at(range.start, &mut bytes)?;
            output.push((slot, bytes));
        }
        if self.check_freshness && self.identity.is_some() {
            self.check_current()?;
        }
        Ok((output, total as u64))
    }
}
/// Classify an error from a source reader. Call this only for open/read errors,
/// after resource and cancellation checks have completed. The metadata probe
/// happens solely on the error path and never accepts new bytes as old ranges.
#[cfg(test)]
pub(crate) fn classify_source_read_failure(
    path: &Path,
    expected: SourceRevision,
    range: Option<ByteRange>,
) -> SourceUnavailableKind {
    probe_source_unavailable(path, expected, range).unwrap_or(SourceUnavailableKind::Unreadable)
}

/// A metadata/open probe for distinguishing source unavailability from parse,
/// schema, and programming failures after a composite operation returned an
/// untyped error. It runs only on that operation's error path.
#[cfg(test)]
pub(crate) fn probe_source_unavailable(
    path: &Path,
    expected: SourceRevision,
    range: Option<ByteRange>,
) -> Option<SourceUnavailableKind> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(SourceUnavailableKind::Missing);
        }
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let actual = match SourceRevision::from_metadata(&metadata) {
        Ok(actual) => actual,
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    if actual != expected {
        return Some(SourceUnavailableKind::RevisionMismatch);
    }
    if range.is_some_and(|range| range.start > range.end || range.end > expected.size) {
        return Some(SourceUnavailableKind::UnusableRange);
    }
    None
}

/// Classify a lifecycle read failure using the same file-incarnation proof as
/// the checked reader. A same-size, same-mtime replacement is a mismatch.
#[cfg(test)]
pub(crate) fn classify_source_read_failure_checked(
    path: &Path,
    expected: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: Option<ByteRange>,
) -> SourceUnavailableKind {
    probe_source_unavailable_checked(path, expected, created_ns, fingerprint, range)
        .unwrap_or(SourceUnavailableKind::Unreadable)
}

#[cfg(test)]
pub(crate) fn probe_source_unavailable_checked(
    path: &Path,
    expected: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: Option<ByteRange>,
) -> Option<SourceUnavailableKind> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(SourceUnavailableKind::Missing);
        }
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let proof = SourceIdentityProof {
        created_ns,
        fingerprint,
    };
    match opened_identity_matches(&mut file, expected, proof) {
        Ok(true) => {}
        Ok(false) => return Some(SourceUnavailableKind::RevisionMismatch),
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    }
    if range.is_some_and(|range| range.start > range.end || range.end > expected.size) {
        return Some(SourceUnavailableKind::UnusableRange);
    }
    None
}

#[cfg(test)]
mod tests;
