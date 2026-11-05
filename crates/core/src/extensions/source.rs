//! Selected source identities and reads.

use super::*;


/// A source's stable identity is distinct from its locator and record ID.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceDescriptor {
    pub collection: String,
    pub provider: String,
    pub source_id: String,
    pub record_id: String,
    pub locator: PathBuf,
    pub format: String,
    pub revision: String,
    pub metadata: BTreeMap<String, String>,
}


/// Enumeration and bounded, revision-checked reads of selected sources.
/// Implementations own their secure-read policy; the engine owns inventories.
pub trait SourceProvider: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    /// Collection-level selector compiled from immutable configuration. A
    /// false answer promises `enumerate` would return no sources there.
    fn may_select(&self, _collection: &str) -> bool {
        true
    }

    fn enumerate(&self, collection: &str) -> Result<Vec<SourceDescriptor>>;
    fn read(&self, source: &SourceDescriptor, output: &mut Vec<u8>) -> Result<()>;
}
