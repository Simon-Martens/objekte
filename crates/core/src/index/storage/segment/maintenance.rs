//! Source-free segment projection requests.

use std::collections::HashSet;

use crate::inventory::ActiveRecords;
use crate::model::FileId;

/// Local live slots and file rows selected for one output segment.
pub(crate) struct SegmentPartition {
    pub files: HashSet<FileId>,
    pub slots: ActiveRecords,
}
