//! Public file provenance values. The catalog service is introduced separately.

use std::path::PathBuf;

use crate::inventory::{ByteRange, FileEntry, FileStatus, SourceRevision};
use crate::model::FileId;

/// File origin resolved when inspecting a record, rather than stored as a
/// copied path in every record header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordFileInfo {
    pub file_id: FileId,
    pub path: PathBuf,
    pub revision: SourceRevision,
    pub created_ns: Option<i128>,
    pub status: FileStatus,
    /// Half-open byte range. `None` is a path-only association.
    pub range: Option<ByteRange>,
}

impl RecordFileInfo {
    pub(crate) fn from_entry(file: &FileEntry, range: Option<ByteRange>) -> Self {
        Self {
            file_id: file.id,
            path: file.relative_path.to_path_buf(),
            revision: file.revision,
            created_ns: file.created_ns,
            status: file.status,
            range,
        }
    }
}
