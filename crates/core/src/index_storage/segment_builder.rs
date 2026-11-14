//! Seal one complete frozen search contribution.

use super::bigram_builder::{build_bigrams, BigramInput};
use super::coverage::build_coverage;
use super::error::{StorageError, StorageResult};
use super::format::{
    BuiltObject, ObjectBuilder, ObjectKind, SectionKind,
};
use super::posting_builder::{build_postings, FieldInput};
use super::records::{
    build_record_table, encode_stats, FilePartInput, RecordInput, SegmentStats,
};
use super::relations::{build_relations, RelationInput};
use super::sort::{build_sort_sections, SortColumnInput};

/// Everything needed to seal one frozen segment.
pub struct SegmentInput {
    pub collection: u32,
    pub source_id: String,
    pub source_version: u32,
    pub file_parts: Vec<FilePartInput>,
    pub records: Vec<RecordInput>,
    pub identity_order: Vec<u32>,
    pub fields: Vec<FieldInput>,
    pub bigrams: Vec<BigramInput>,
    pub sorts: Vec<SortColumnInput>,
    pub relations: Vec<RelationInput>,
    pub logical_records: Vec<u32>,
    pub empty_arrays: Vec<u32>,
    pub estimated_index_bytes: u64,
}

/// One built object plus its decoded statistics.
pub struct BuiltSegment {
    pub object: BuiltObject,
    pub stats: SegmentStats,
}

/// Build one complete frozen segment.
pub fn build_segment(input: SegmentInput) -> StorageResult<BuiltSegment> {
    let slot_count = u32::try_from(input.records.len())
        .map_err(|_| StorageError::ResourceLimit("segment record count exceeds u32".into()))?;
    let record_table = build_record_table(
        &input.file_parts,
        &input.records,
        &input.identity_order,
        &input.source_id,
        slot_count,
    )?;
    let mut string_arena = record_table.string_arena;
    let postings = build_postings(&input.fields, slot_count)?;
    let bigrams = build_bigrams(&input.bigrams, slot_count)?;
    let sorts = build_sort_sections(&input.sorts, slot_count)?;
    let relations = build_relations(&input.relations, slot_count, &mut string_arena)?;
    let coverage = build_coverage(&input.logical_records, &input.empty_arrays, slot_count)?;
    let stats = SegmentStats {
        collection: input.collection,
        source_version: input.source_version,
        source_id: input.source_id,
        source_id_offset: record_table.source_id_offset,
        source_id_len: record_table.source_id_len,
        slot_count,
        record_count: slot_count,
        file_part_count: input.file_parts.len() as u32,
        identity_count: input.identity_order.len() as u32,
        field_count: input.fields.len() as u32,
        bigram_binding_count: input.bigrams.len() as u32,
        sort_column_count: input.sorts.len() as u32,
        relation_count: input.relations.len() as u32,
        fallback_count: record_table.fallback_count,
        source_bytes: record_table.source_bytes,
        estimated_index_bytes: input.estimated_index_bytes,
    };
    let statistics = encode_stats(&stats);
    let mut builder = ObjectBuilder::new(ObjectKind::Segment);
    builder.add_fixed_section(
        SectionKind::Records,
        super::records::RECORD_ROW_LEN as u16,
        record_table.records,
        slot_count as u64,
    )?;
    builder.add_fixed_section(
        SectionKind::FileParts,
        super::records::FILE_PART_ROW_LEN as u16,
        record_table.file_parts,
        input.file_parts.len() as u64,
    )?;
    builder.add_section(SectionKind::LogicalIds, record_table.logical_ids)?;
    builder.add_fixed_section(
        SectionKind::IdentityOrder,
        4,
        record_table.identity_order,
        input.identity_order.len() as u64,
    )?;
    builder.add_section(SectionKind::FieldDirectory, postings.field_directory)?;
    builder.add_section(SectionKind::DictionaryPages, Vec::new())?;
    builder.add_section(SectionKind::PostingPayloads, postings.payloads)?;
    builder.add_section(SectionKind::BigramDictionaries, bigrams.dictionaries)?;
    builder.add_section(SectionKind::BigramPostings, bigrams.postings)?;
    builder.add_section(SectionKind::SortKeys, sorts.keys)?;
    builder.add_section(SectionKind::SortOrders, sorts.orders)?;
    builder.add_section(SectionKind::Relations, relations)?;
    builder.add_section(SectionKind::StringArena, string_arena)?;
    builder.add_section(SectionKind::Coverage, coverage)?;
    builder.add_section(SectionKind::Statistics, statistics)?;
    let object = builder.finish()?;
    Ok(BuiltSegment { object, stats })
}

