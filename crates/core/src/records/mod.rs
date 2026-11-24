//! Source-neutral logical record identity and public metadata.

use serde::{Deserialize, Serialize};

use crate::files::RecordFileInfo;
use crate::inventory::{DiagnosticRef, RecordClass};
use crate::model::{CollectionId, RecordId};

pub mod reader;

/// A logical record key. IDs are unique among live records only within their
/// collection; a segment slot is a separate physical address in a pinned view.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordKey {
    pub collection: CollectionId,
    pub record: RecordId,
}

impl RecordKey {
    pub fn new(collection: CollectionId, record: RecordId) -> Self {
        Self { collection, record }
    }
}

/// Provider-issued version token scoped to a registration and record key.
/// A reader must reject this token if its exact version is no longer available.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RecordVersion(u64);

impl RecordVersion {
    pub const fn new(token: u64) -> Self {
        Self(token)
    }

    pub const fn token(self) -> u64 {
        self.0
    }
}

/// Public metadata resolved from one immutable publication. File origin is
/// optional even when the index has a registered file catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordMetadata {
    pub key: RecordKey,
    pub generation: u64,
    pub class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
    pub file: Option<RecordFileInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_external_id_in_two_collections_has_two_keys() {
        let id = RecordId::intrinsic("entry-47").unwrap();
        assert_ne!(
            RecordKey::new(CollectionId(1), id.clone()),
            RecordKey::new(CollectionId(2), id)
        );
    }
}
