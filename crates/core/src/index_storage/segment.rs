//! Frozen segment readers over packed object bytes.

use super::bigram::BigramSectionView;
use super::coverage::CoverageView;
use super::error::{StorageError, StorageResult};
use super::format::{ObjectHeader, ObjectKind, ObjectView, SectionDescriptor, SectionKind};
use super::posting::PostingSectionView;
use super::records::{RecordTableView, SegmentStats, decode_stats};
use super::relations::RelationSectionView;
use super::sort::SortSectionView;


/// A fully opened frozen segment with lightweight readers.
pub struct FrozenSegment {
    backing: super::backing::Backing,
    header: ObjectHeader,
    stats: SegmentStats,
    records: RecordTableView,
    postings: PostingSectionView,
    bigrams: BigramSectionView,
    sorting: SortSectionView,
    relations: RelationSectionView,
    coverage: CoverageView,
    metadata: Option<SectionDescriptor>,
}


impl std::fmt::Debug for FrozenSegment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FrozenSegment")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}


impl FrozenSegment {
    pub fn open(backing: super::backing::Backing) -> StorageResult<Self> {
        let view = ObjectView::new(&backing)?;
        if view.header().kind != ObjectKind::Segment {
            return Err(StorageError::Incompatible(
                "object is not a frozen segment".into(),
            ));
        }
        let header = view.header();
        let sections = view
            .sections()
            .iter()
            .filter_map(|descriptor| descriptor.kind.map(|kind| (kind, *descriptor)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let statistics = sections
            .get(&SectionKind::Statistics)
            .copied()
            .ok_or_else(|| StorageError::corrupt("segment statistics are missing"))?;
        let mut stats = decode_stats(backing.read(statistics.offset, statistics.length)?)?;
        let records = RecordTableView::open(backing.clone(), sections.clone(), stats.clone())?;
        stats = records.stats().clone();
        let posting_desc = sections
            .get(&SectionKind::PostingPayloads)
            .copied()
            .ok_or_else(|| StorageError::corrupt("posting payloads are missing"))?;
        let field_directory = sections
            .get(&SectionKind::FieldDirectory)
            .copied()
            .ok_or_else(|| StorageError::corrupt("field directory is missing"))?;
        let postings = PostingSectionView::open(
            backing.clone(),
            posting_desc,
            field_directory,
            stats.slot_count,
        )?;
        let bigram_dicts = sections
            .get(&SectionKind::BigramDictionaries)
            .copied()
            .ok_or_else(|| StorageError::corrupt("bigram dictionaries are missing"))?;
        let bigram_posts = sections
            .get(&SectionKind::BigramPostings)
            .copied()
            .ok_or_else(|| StorageError::corrupt("bigram postings are missing"))?;
        let bigrams = BigramSectionView::open(
            backing.clone(),
            bigram_dicts,
            bigram_posts,
            stats.slot_count,
        )?;
        let sort_keys = sections
            .get(&SectionKind::SortKeys)
            .copied()
            .ok_or_else(|| StorageError::corrupt("sort keys are missing"))?;
        let sort_orders = sections
            .get(&SectionKind::SortOrders)
            .copied()
            .ok_or_else(|| StorageError::corrupt("sort orders are missing"))?;
        let sorting =
            SortSectionView::open(backing.clone(), sort_keys, sort_orders, stats.slot_count)?;
        let relation_payloads = sections
            .get(&SectionKind::Relations)
            .copied()
            .ok_or_else(|| StorageError::corrupt("relations are missing"))?;
        let string_arena = sections
            .get(&SectionKind::StringArena)
            .copied()
            .ok_or_else(|| StorageError::corrupt("string arena is missing"))?;
        let relations = RelationSectionView::open(
            backing.clone(),
            relation_payloads,
            string_arena,
            stats.slot_count,
            stats.relation_count,
        )?;
        let coverage_desc = sections
            .get(&SectionKind::Coverage)
            .copied()
            .ok_or_else(|| StorageError::corrupt("coverage is missing"))?;
        let coverage = CoverageView::open(backing.clone(), coverage_desc, stats.slot_count)?;
        let metadata = sections.get(&SectionKind::Compatibility).copied();
        Ok(Self {
            backing,
            header,
            stats,
            records,
            postings,
            bigrams,
            sorting,
            relations,
            coverage,
            metadata,
        })
    }

    pub fn from_bytes(bytes: impl Into<std::sync::Arc<[u8]>>) -> StorageResult<Self> {
        Self::open(super::backing::Backing::resident(bytes.into()))
    }

    pub fn open_path(path: &std::path::Path) -> StorageResult<Self> {
        let object = std::sync::Arc::new(super::backing::MappedObject::open(path, None)?);
        Self::open(super::backing::Backing::mapped(object))
    }

    pub fn stats(&self) -> &SegmentStats {
        &self.stats
    }

    pub fn header(&self) -> ObjectHeader {
        self.header
    }

    pub fn records(&self) -> &RecordTableView {
        &self.records
    }

    pub fn postings(&self) -> &PostingSectionView {
        &self.postings
    }

    pub fn bigrams(&self) -> &BigramSectionView {
        &self.bigrams
    }

    pub fn sorting(&self) -> &SortSectionView {
        &self.sorting
    }

    pub fn relations(&self) -> &RelationSectionView {
        &self.relations
    }

    pub fn coverage(&self) -> &CoverageView {
        &self.coverage
    }

    pub fn metadata(&self) -> StorageResult<Option<&[u8]>> {
        self.metadata
            .map(|descriptor| self.backing.read(descriptor.offset, descriptor.length))
            .transpose()
    }

    pub fn backing(&self) -> &super::backing::Backing {
        &self.backing
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        ObjectView::new(&self.backing)?.verify_full()?;
        self.postings.verify_full()?;
        self.bigrams.verify_full()?;
        self.sorting.verify_full()?;
        self.relations.verify_full()?;
        self.coverage.verify_full()?;
        for slot in 0..self.stats.slot_count {
            self.records.record(slot)?;
        }
        Ok(())
    }
}
