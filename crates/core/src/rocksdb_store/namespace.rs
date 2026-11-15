//! One-byte logical namespaces used ahead of ordered logical keys.

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Namespace {
    StoreMetadata = 0x01,
    Collections = 0x02,
    Fields = 0x03,
    Bindings = 0x04,
    Files = 0x05,
    FileBuildState = 0x06,
    PendingFiles = 0x07,
    Paths = 0x08,
    QueryPaths = 0x09,
    Records = 0x0A,
    LogicalIds = 0x0B,
    FileRecords = 0x0C,
    SlotIncarnations = 0x0D,
    FreeSlots = 0x0E,
    RecordContributions = 0x0F,
    ValueBlocks = 0x10,
    LengthBlocks = 0x11,
    ValueMembers = 0x12,
    LengthMembers = 0x13,
    StateBlocks = 0x14,
    StateStatistics = 0x15,
    ValueDirectory = 0x16,
    LengthDirectory = 0x17,
    FieldCoverage = 0x18,
    CoverageMembers = 0x19,
    DomainMembers = 0x1A,
    SortDefinitions = 0x1B,
    SortEntries = 0x1C,
    RelationOutgoing = 0x1D,
    RelationIncoming = 0x1E,
    RelationStates = 0x1F,
    FtsBlocks = 0x20,
    FtsGramStatistics = 0x21,
    RecordGrams = 0x22,
    BuildReceipts = 0x40,
    FieldCoverageDeltas = 0x41,
}

impl Namespace {
    pub const ALL: &'static [Self] = &[
        Self::StoreMetadata,
        Self::Collections,
        Self::Fields,
        Self::Bindings,
        Self::Files,
        Self::FileBuildState,
        Self::PendingFiles,
        Self::Paths,
        Self::QueryPaths,
        Self::Records,
        Self::LogicalIds,
        Self::FileRecords,
        Self::SlotIncarnations,
        Self::FreeSlots,
        Self::RecordContributions,
        Self::ValueBlocks,
        Self::LengthBlocks,
        Self::ValueMembers,
        Self::LengthMembers,
        Self::StateBlocks,
        Self::StateStatistics,
        Self::ValueDirectory,
        Self::LengthDirectory,
        Self::FieldCoverage,
        Self::CoverageMembers,
        Self::DomainMembers,
        Self::SortDefinitions,
        Self::SortEntries,
        Self::RelationOutgoing,
        Self::RelationIncoming,
        Self::RelationStates,
        Self::FtsBlocks,
        Self::FtsGramStatistics,
        Self::RecordGrams,
        Self::BuildReceipts,
        Self::FieldCoverageDeltas,
    ];

    pub const fn tag(self) -> u8 {
        self as u8
    }

    pub fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            0x01 => Self::StoreMetadata,
            0x02 => Self::Collections,
            0x03 => Self::Fields,
            0x04 => Self::Bindings,
            0x05 => Self::Files,
            0x06 => Self::FileBuildState,
            0x07 => Self::PendingFiles,
            0x08 => Self::Paths,
            0x09 => Self::QueryPaths,
            0x0A => Self::Records,
            0x0B => Self::LogicalIds,
            0x0C => Self::FileRecords,
            0x0D => Self::SlotIncarnations,
            0x0E => Self::FreeSlots,
            0x0F => Self::RecordContributions,
            0x10 => Self::ValueBlocks,
            0x11 => Self::LengthBlocks,
            0x12 => Self::ValueMembers,
            0x13 => Self::LengthMembers,
            0x14 => Self::StateBlocks,
            0x15 => Self::StateStatistics,
            0x16 => Self::ValueDirectory,
            0x17 => Self::LengthDirectory,
            0x18 => Self::FieldCoverage,
            0x19 => Self::CoverageMembers,
            0x1A => Self::DomainMembers,
            0x1B => Self::SortDefinitions,
            0x1C => Self::SortEntries,
            0x1D => Self::RelationOutgoing,
            0x1E => Self::RelationIncoming,
            0x1F => Self::RelationStates,
            0x20 => Self::FtsBlocks,
            0x21 => Self::FtsGramStatistics,
            0x22 => Self::RecordGrams,
            0x40 => Self::BuildReceipts,
            0x41 => Self::FieldCoverageDeltas,
            _ => return None,
        })
    }

    pub fn physical(self, logical_key: &[u8]) -> Vec<u8> {
        let mut key = Vec::with_capacity(logical_key.len() + 1);
        key.push(self.tag());
        key.extend_from_slice(logical_key);
        key
    }
}

pub fn namespace_of(physical_key: &[u8]) -> Option<Namespace> {
    physical_key
        .first()
        .copied()
        .and_then(Namespace::from_tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn registry_tags_are_unique_and_round_trip() {
        let mut tags = BTreeSet::new();
        for namespace in Namespace::ALL {
            assert!(tags.insert(namespace.tag()));
            assert_eq!(Namespace::from_tag(namespace.tag()), Some(*namespace));
        }
        assert_eq!(Namespace::from_tag(0), None);
        assert_eq!(Namespace::from_tag(0xFF), None);
    }

    #[test]
    fn physical_keys_include_the_exact_namespace_byte() {
        let key = Namespace::Records.physical(&[0xDE, 0xAD]);
        assert_eq!(key, [0x0A, 0xDE, 0xAD]);
        assert_eq!(namespace_of(&key), Some(Namespace::Records));
        assert_eq!(namespace_of(&[]), None);
    }
}
