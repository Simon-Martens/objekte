//! Source-I/O-free packing of one immutable search segment.
//!
//! Version 1 keeps typed dictionary bytes and portable Roaring postings in
//! independently addressable sections. The materialized and sealed decoders
//! share all metadata validation. The sealed path keeps typed dictionary
//! readers and checks posting payloads only when queries access them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Seek, Write};
use std::sync::Arc;

use bincode::Options;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::bigram::BigramIndex;
use crate::bigram_dictionary::{BigramSpans, append_bigram_index};
use crate::build::IndexedCollectionSegment;
use crate::inference::{InferenceMode, InferenceReport, InferenceState};
use crate::inventory::FileEntry;
use crate::model::{CollectionId, LogicalType, Number};
use crate::ordered_block::OrderedBlockDictionary;
use crate::posting::{
    CustomPostingIndex, PostingCollection, PostingIndex, PostingStats, ScalarKey, StructuralFacts,
    StructuralKey,
};
use crate::posting_dictionary::{
    DictionarySpan, PostingPack, SealedPostingDictionary, append_dictionary,
};
use crate::repository::{CollectionDefinition, RepositoryOptions, canonical_custom_text_key};
use crate::segment_format::{
    PostingDescriptor, REQUIRED_SECTIONS, SectionKind, SegmentReader, SegmentWriter,
};
use crate::segment_metadata::{
    decode_record_slots, decode_relation_identity, decode_sort_data, encode_record_slots,
    encode_relation_identity, encode_sort_data,
};
use crate::workload::BigramAlphabet;

const META_VERSION: u32 = 1;
const FIELD_MAGIC: &[u8; 8] = b"OBJHFLD1";
const STRUCT_MAGIC: &[u8; 8] = b"OBJHSTR1";
const TEXT_MAGIC: &[u8; 8] = b"OBJHTXT1";
const INTEGRITY_MAGIC: &[u8; 8] = b"OBJHINT1";
const MAX_META_BYTES: usize = 512 * 1024 * 1024;
const MAX_BINDINGS: usize = 65_536;

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

fn encode_meta<T: Serialize>(magic: &[u8; 8], value: &T) -> Result<Vec<u8>, String> {
    let body = codec()
        .serialize(value)
        .map_err(|error| error.to_string())?;
    if body.len() > MAX_META_BYTES - 16 {
        return Err("hybrid metadata section exceeds limit".into());
    }
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&META_VERSION.to_le_bytes());
    out.extend_from_slice(&crate::MODEL_VERSION.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn decode_meta<T: DeserializeOwned>(magic: &[u8; 8], bytes: &[u8]) -> Result<T, String> {
    if bytes.len() < 16
        || bytes.len() > MAX_META_BYTES
        || &bytes[..8] != magic
        || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != META_VERSION
        || u32::from_le_bytes(bytes[12..16].try_into().unwrap()) != crate::MODEL_VERSION
    {
        return Err("hybrid metadata magic, version, or size mismatch".into());
    }
    codec()
        .with_limit((bytes.len() - 16) as u64)
        .deserialize(&bytes[16..])
        .map_err(|error| format!("invalid hybrid metadata: {error}"))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Span {
    start: u64,
    length: u64,
}

impl From<DictionarySpan> for Span {
    fn from(span: DictionarySpan) -> Self {
        Self {
            start: span.start as u64,
            length: span.length as u64,
        }
    }
}

impl Span {
    fn checked(self) -> Result<DictionarySpan, String> {
        Ok(DictionarySpan {
            start: usize::try_from(self.start).map_err(|_| "dictionary offset overflow")?,
            length: usize::try_from(self.length).map_err(|_| "dictionary length overflow")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Descriptor {
    offset: u64,
    length: u32,
    cardinality: u32,
    checksum: u64,
}

impl From<PostingDescriptor> for Descriptor {
    fn from(value: PostingDescriptor) -> Self {
        Self {
            offset: value.offset,
            length: value.length,
            cardinality: value.cardinality,
            checksum: value.checksum,
        }
    }
}

impl From<Descriptor> for PostingDescriptor {
    fn from(value: Descriptor) -> Self {
        Self {
            offset: value.offset,
            length: value.length,
            cardinality: value.cardinality,
            checksum: value.checksum,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
enum PostingKey {
    Declared(Vec<String>),
    Named(Vec<String>, String),
    Inferred(Vec<String>),
}

#[derive(Deserialize, Serialize)]
struct FieldBinding {
    key: PostingKey,
    kind: LogicalType,
    multi: bool,
    value_complete: bool,
    length_complete: bool,
    empty_string_complete: bool,
    covered_records: u32,
    stats: PostingStats,
    custom_complete: Option<bool>,
    values: Span,
    lengths: Span,
}

#[derive(Deserialize, Serialize)]
struct FieldSection {
    bindings: Vec<FieldBinding>,
    inference_report: Option<InferenceReport>,
    record_parses: u64,
    failed_parses: u64,
}

#[derive(Deserialize, Serialize)]
struct StructuralBinding {
    key: PostingKey,
    presence: Descriptor,
    empty_selections: Descriptor,
    nulls: Descriptor,
    facts: Option<Vec<(StructuralKey, Descriptor)>>,
}

#[derive(Deserialize, Serialize)]
struct StructuralSection {
    bindings: Vec<StructuralBinding>,
    empty_array_records: Descriptor,
    inferred_empty_selections: Vec<(Vec<String>, Descriptor)>,
    short_string_coverage: Vec<(Vec<String>, Descriptor)>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
enum TextKey {
    Raw,
    Fallback,
    Named(Vec<String>, String),
}

#[derive(Deserialize, Serialize)]
struct TextBinding {
    key: TextKey,
    alphabet: BigramAlphabet,
    record_count: u32,
    normal: Span,
    skip: Span,
}

#[derive(Deserialize, Serialize)]
struct TextSection {
    bindings: Vec<TextBinding>,
}

#[derive(Deserialize, Serialize)]
struct IntegritySection {
    collection: CollectionId,
    ordinal: u64,
    source_id: String,
    source_version: u32,
    slot_count: u32,
    alphabet: BigramAlphabet,
    field_bindings: u32,
    text_bindings: u32,
}

fn posting_inputs(
    segment: &IndexedCollectionSegment,
) -> Vec<(PostingKey, &PostingIndex, Option<bool>)> {
    let mut out = Vec::new();
    out.extend(
        segment
            .postings
            .fields
            .iter()
            .map(|(path, index)| (PostingKey::Declared(path.clone()), index, None)),
    );
    out.extend(
        segment
            .custom_postings
            .iter()
            .map(|((path, binding), custom)| {
                (
                    PostingKey::Named(path.clone(), binding.clone()),
                    &custom.index,
                    Some(custom.complete),
                )
            }),
    );
    if let Some(inference) = &segment.inference {
        out.extend(
            inference
                .indexes
                .iter()
                .map(|(path, index)| (PostingKey::Inferred(path.clone()), index, None)),
        );
    }
    out.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    out
}

fn text_inputs(segment: &IndexedCollectionSegment) -> Vec<(TextKey, &BigramIndex)> {
    let mut out = vec![
        (TextKey::Raw, &segment.raw),
        (TextKey::Fallback, &segment.fallback_text),
    ];
    out.extend(
        segment
            .custom_text
            .iter()
            .map(|((path, binding), index)| (TextKey::Named(path.clone(), binding.clone()), index)),
    );
    out.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    out
}

fn descriptor_list(
    pack: &mut PostingPack,
    values: &HashMap<Vec<String>, RoaringBitmap>,
) -> Result<Vec<(Vec<String>, Descriptor)>, String> {
    let mut ordered = values.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| left.0.cmp(right.0));
    ordered
        .into_iter()
        .map(|(key, bitmap)| Ok((key.clone(), pack.append(bitmap)?.into())))
        .collect()
}

fn write_bytes<W: Write + Seek>(
    writer: &mut SegmentWriter<W>,
    kind: SectionKind,
    bytes: &[u8],
) -> Result<(), String> {
    writer.write_section(kind, |out| {
        out.write_all(bytes).map_err(|error| error.to_string())
    })
}

/// Encode a complete collection segment into the ten-section immutable format.
/// The callback interns file metadata in the generation catalog, including
/// distinct historical revisions, and returns its checked table ID.
pub(crate) fn write_segment<W: Write + Seek>(
    segment: &IndexedCollectionSegment,
    ordinal: u64,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    output: W,
    intern: &mut dyn FnMut(&FileEntry) -> Result<u32, String>,
) -> Result<W, String> {
    if ordinal == 0 {
        return Err("hybrid segment ordinal must be nonzero".into());
    }
    validate_components(segment, definition, options)?;
    let slot_count = u32::try_from(segment.records.len()).map_err(|_| "too many record slots")?;
    let mut pack = PostingPack::new();
    let mut scalar = Vec::new();
    let mut bigrams = Vec::new();
    let mut fields = Vec::new();
    let mut structural = Vec::new();
    for (key, index, custom_complete) in posting_inputs(segment) {
        let values = append_dictionary(&mut scalar, &mut pack, &index.values)?;
        let lengths = append_dictionary(&mut scalar, &mut pack, &index.lengths)?;
        let facts = index
            .structural
            .as_ref()
            .map(|facts| {
                facts
                    .postings
                    .iter()
                    .map(|(key, bitmap)| Ok((*key, pack.append(bitmap)?.into())))
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?;
        structural.push(StructuralBinding {
            key: key.clone(),
            presence: pack.append(&index.presence)?.into(),
            empty_selections: pack.append(&index.empty_selections)?.into(),
            nulls: pack.append(&index.nulls)?.into(),
            facts,
        });
        fields.push(FieldBinding {
            key,
            kind: index.kind.clone(),
            multi: index.multi,
            value_complete: index.value_complete,
            length_complete: index.length_complete,
            empty_string_complete: index.empty_string_complete,
            covered_records: index.covered_records,
            stats: index.stats.clone(),
            custom_complete,
            values: values.into(),
            lengths: lengths.into(),
        });
    }
    let fallback = segment
        .records
        .iter()
        .filter(|record| record.class == crate::inventory::RecordClass::RawFallback)
        .map(|record| record.slot.0)
        .collect::<RoaringBitmap>();
    let mut texts = Vec::new();
    for (key, index) in text_inputs(segment) {
        let allowed = if key == TextKey::Fallback {
            &fallback
        } else {
            &segment.logical_records
        };
        let spans = append_bigram_index(&mut bigrams, &mut pack, index, allowed)?;
        texts.push(TextBinding {
            key,
            alphabet: index.alphabet,
            record_count: index.record_count,
            normal: spans.normal.into(),
            skip: spans.skip.into(),
        });
    }
    let field_bytes = encode_meta(
        FIELD_MAGIC,
        &FieldSection {
            bindings: fields,
            inference_report: segment.inference.as_ref().map(|state| state.report.clone()),
            record_parses: segment.record_parses,
            failed_parses: segment.failed_parses,
        },
    )?;
    let structural_bytes = encode_meta(
        STRUCT_MAGIC,
        &StructuralSection {
            bindings: structural,
            empty_array_records: pack.append(&segment.empty_array_records)?.into(),
            inferred_empty_selections: segment
                .inference
                .as_ref()
                .map(|state| descriptor_list(&mut pack, &state.empty_selections))
                .transpose()?
                .unwrap_or_default(),
            short_string_coverage: segment
                .inference
                .as_ref()
                .map(|state| descriptor_list(&mut pack, &state.short_string_coverage))
                .transpose()?
                .unwrap_or_default(),
        },
    )?;
    let text_bytes = encode_meta(TEXT_MAGIC, &TextSection { bindings: texts })?;
    let record_bytes = encode_record_slots(segment, intern)?;
    let sort_bytes = encode_sort_data(&segment.sorting, &segment.records)?;
    let relation_bytes = encode_relation_identity(
        &segment.relations,
        &segment.records,
        &segment.identity_order,
    )?;
    let integrity_bytes = encode_meta(
        INTEGRITY_MAGIC,
        &IntegritySection {
            collection: segment.collection,
            ordinal,
            source_id: segment.source_id.clone(),
            source_version: segment.source_version,
            slot_count,
            alphabet: options.bigram_alphabet,
            field_bindings: u32::try_from(posting_inputs(segment).len())
                .map_err(|_| "too many field bindings")?,
            text_bindings: u32::try_from(text_inputs(segment).len())
                .map_err(|_| "too many text bindings")?,
        },
    )?;
    let mut writer = SegmentWriter::new(output, REQUIRED_SECTIONS.len())?;
    write_bytes(&mut writer, SectionKind::FieldMetadata, &field_bytes)?;
    write_bytes(&mut writer, SectionKind::TextBindings, &text_bytes)?;
    write_bytes(&mut writer, SectionKind::ScalarDictionaries, &scalar)?;
    write_bytes(
        &mut writer,
        SectionKind::StructuralReferences,
        &structural_bytes,
    )?;
    write_bytes(&mut writer, SectionKind::BigramDictionaries, &bigrams)?;
    write_bytes(&mut writer, SectionKind::PostingPayloads, pack.bytes())?;
    write_bytes(&mut writer, SectionKind::RecordSlots, &record_bytes)?;
    write_bytes(&mut writer, SectionKind::SortData, &sort_bytes)?;
    write_bytes(
        &mut writer,
        SectionKind::RelationAndIdentity,
        &relation_bytes,
    )?;
    write_bytes(&mut writer, SectionKind::Integrity, &integrity_bytes)?;
    writer.finish()
}

fn required<'a>(reader: &'a SegmentReader, kind: SectionKind) -> Result<&'a [u8], String> {
    reader
        .section(kind)?
        .ok_or_else(|| format!("missing hybrid section {kind:?}"))
}

fn materialize(
    reader: &SegmentReader,
    descriptor: Descriptor,
    allowed: &RoaringBitmap,
) -> Result<RoaringBitmap, String> {
    reader.materialize_posting(descriptor.into(), allowed)
}

fn materialize_dictionary<K: Ord + Clone + DeserializeOwned>(
    reader: &Arc<SegmentReader>,
    section: SectionKind,
    span: Span,
    allowed: &RoaringBitmap,
) -> Result<BTreeMap<K, RoaringBitmap>, String> {
    let span = span.checked()?;
    let dictionary =
        OrderedBlockDictionary::<K>::open(Arc::clone(reader), section, span.start, span.length)?;
    let mut out = BTreeMap::new();
    let mut cursor = dictionary.first();
    while let Some(position) = cursor {
        let (key, descriptor) = dictionary.entry(position)?;
        let bitmap = reader.materialize_posting(descriptor, allowed)?;
        if out.insert(key, bitmap).is_some() {
            return Err("duplicate hybrid dictionary key".into());
        }
        cursor = dictionary.next(position);
    }
    if out.len() != dictionary.len() {
        return Err("hybrid dictionary entry count mismatch".into());
    }
    Ok(out)
}

fn unique_keys<K: Ord + Clone>(keys: impl IntoIterator<Item = K>) -> Result<(), String> {
    let mut previous = None;
    for key in keys {
        if previous.as_ref().is_some_and(|last| last >= &key) {
            return Err("hybrid binding order or uniqueness mismatch".into());
        }
        previous = Some(key);
    }
    Ok(())
}

fn check_spans(spans: impl IntoIterator<Item = Span>, section_len: usize) -> Result<(), String> {
    let mut position = 0u64;
    for span in spans {
        if span.length == 0 || span.start != position {
            return Err("hybrid dictionary span gap or overlap".into());
        }
        position = position
            .checked_add(span.length)
            .ok_or("hybrid dictionary span overflow")?;
        if position > section_len as u64 {
            return Err("hybrid dictionary span out of bounds".into());
        }
    }
    if position != section_len as u64 {
        return Err("hybrid dictionary section has unreferenced bytes".into());
    }
    Ok(())
}

/// Decode into mutable owned maps for compatibility and inspection tests.
/// Every posting payload is validated and materialized before returning.
pub(crate) fn decode_segment_owned<'a>(
    reader: Arc<SegmentReader>,
    expected_ordinal: u64,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    collection: CollectionId,
    source: (&str, u32),
    expected_slot_count: u32,
    resolve: &dyn Fn(u32) -> Option<&'a FileEntry>,
) -> Result<IndexedCollectionSegment, String> {
    decode_segment_impl(
        reader,
        expected_ordinal,
        definition,
        options,
        collection,
        source,
        expected_slot_count,
        resolve,
        false,
    )
}

/// Open a managed segment with checked metadata and immutable dictionary
/// readers. Both mapped and fully resident readers use this same path.
pub(crate) fn decode_segment_sealed<'a>(
    reader: Arc<SegmentReader>,
    expected_ordinal: u64,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    collection: CollectionId,
    source: (&str, u32),
    expected_slot_count: u32,
    resolve: &dyn Fn(u32) -> Option<&'a FileEntry>,
) -> Result<IndexedCollectionSegment, String> {
    decode_segment_impl(
        reader,
        expected_ordinal,
        definition,
        options,
        collection,
        source,
        expected_slot_count,
        resolve,
        true,
    )
}

fn decode_segment_impl<'a>(
    reader: Arc<SegmentReader>,
    expected_ordinal: u64,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
    collection: CollectionId,
    source: (&str, u32),
    expected_slot_count: u32,
    resolve: &dyn Fn(u32) -> Option<&'a FileEntry>,
    sealed: bool,
) -> Result<IndexedCollectionSegment, String> {
    if expected_ordinal == 0 {
        return Err("hybrid segment ordinal must be nonzero".into());
    }
    reader.require_complete_sections()?;
    // Avoid section(PostingPayloads): hashing that whole region defeats
    // on-demand startup. Every referenced descriptor is checked below.
    let field_section: FieldSection =
        decode_meta(FIELD_MAGIC, required(&reader, SectionKind::FieldMetadata)?)?;
    let structure: StructuralSection = decode_meta(
        STRUCT_MAGIC,
        required(&reader, SectionKind::StructuralReferences)?,
    )?;
    let texts: TextSection =
        decode_meta(TEXT_MAGIC, required(&reader, SectionKind::TextBindings)?)?;
    let integrity: IntegritySection =
        decode_meta(INTEGRITY_MAGIC, required(&reader, SectionKind::Integrity)?)?;
    let scalar_len = required(&reader, SectionKind::ScalarDictionaries)?.len();
    let bigram_len = required(&reader, SectionKind::BigramDictionaries)?.len();
    check_spans(
        field_section
            .bindings
            .iter()
            .flat_map(|field| [field.values, field.lengths]),
        scalar_len,
    )?;
    check_spans(
        texts
            .bindings
            .iter()
            .flat_map(|text| [text.normal, text.skip]),
        bigram_len,
    )?;
    let records = decode_record_slots(
        required(&reader, SectionKind::RecordSlots)?,
        collection,
        source,
        resolve,
    )?;
    let count = records.slot_count();
    if count != expected_slot_count
        || integrity.collection != collection
        || integrity.ordinal != expected_ordinal
        || integrity.source_id != source.0
        || integrity.source_version != source.1
        || integrity.slot_count != count
        || integrity.alphabet != options.bigram_alphabet
        || integrity.field_bindings as usize != field_section.bindings.len()
        || integrity.text_bindings as usize != texts.bindings.len()
    {
        return Err("hybrid segment identity or domain mismatch".into());
    }
    if field_section.bindings.len() > MAX_BINDINGS
        || structure.bindings.len() != field_section.bindings.len()
        || texts.bindings.len() > MAX_BINDINGS
    {
        return Err("hybrid binding count mismatch".into());
    }
    unique_keys(
        field_section
            .bindings
            .iter()
            .map(|binding| binding.key.clone()),
    )?;
    unique_keys(structure.bindings.iter().map(|binding| binding.key.clone()))?;
    unique_keys(texts.bindings.iter().map(|binding| binding.key.clone()))?;
    let mut structural = structure
        .bindings
        .into_iter()
        .map(|binding| (binding.key.clone(), binding))
        .collect::<HashMap<_, _>>();
    let logical = &records.logical_records;
    let fallback = &records.fallback_records;
    let logical_shared = sealed.then(|| Arc::new(logical.clone()));
    let fallback_shared = sealed.then(|| Arc::new(fallback.clone()));
    let mut declared = HashMap::new();
    let mut named = HashMap::new();
    let mut inferred = HashMap::new();
    for field in field_section.bindings {
        let refs = structural
            .remove(&field.key)
            .ok_or("hybrid structural binding missing")?;
        let facts = refs
            .facts
            .map(|facts| {
                let mut postings = BTreeMap::new();
                for (key, descriptor) in facts {
                    if postings
                        .insert(key, materialize(&reader, descriptor, logical)?)
                        .is_some()
                    {
                        return Err("duplicate structural fact".into());
                    }
                }
                Ok::<StructuralFacts, String>(StructuralFacts { postings })
            })
            .transpose()?;
        let path = match &field.key {
            PostingKey::Declared(path)
            | PostingKey::Inferred(path)
            | PostingKey::Named(path, _) => path.clone(),
        };
        let mut index = PostingIndex {
            path,
            kind: field.kind,
            multi: field.multi,
            presence: materialize(&reader, refs.presence, logical)?,
            empty_selections: materialize(&reader, refs.empty_selections, logical)?,
            nulls: materialize(&reader, refs.nulls, logical)?,
            structural: facts,
            values: if sealed {
                BTreeMap::new()
            } else {
                materialize_dictionary::<ScalarKey>(
                    &reader,
                    SectionKind::ScalarDictionaries,
                    field.values,
                    logical,
                )?
            },
            lengths: if sealed {
                BTreeMap::new()
            } else {
                materialize_dictionary::<Number>(
                    &reader,
                    SectionKind::ScalarDictionaries,
                    field.lengths,
                    logical,
                )?
            },
            sealed: None,
            value_complete: field.value_complete,
            length_complete: field.length_complete,
            empty_string_complete: field.empty_string_complete,
            covered_records: field.covered_records,
            stats: field.stats,
        };
        if sealed {
            let allowed = Arc::clone(logical_shared.as_ref().unwrap());
            index.install_sealed(
                SealedPostingDictionary::open(
                    Arc::clone(&reader),
                    SectionKind::ScalarDictionaries,
                    field.values.checked()?,
                    Arc::clone(&allowed),
                )?,
                SealedPostingDictionary::open(
                    Arc::clone(&reader),
                    SectionKind::ScalarDictionaries,
                    field.lengths.checked()?,
                    allowed,
                )?,
            )?;
        }
        match field.key {
            PostingKey::Declared(path) => {
                if field.custom_complete.is_some() || declared.insert(path, index).is_some() {
                    return Err("declared posting binding mismatch".into());
                }
            }
            PostingKey::Named(path, binding) => {
                let complete = field
                    .custom_complete
                    .ok_or("named posting completeness missing")?;
                if named
                    .insert((path, binding), CustomPostingIndex { index, complete })
                    .is_some()
                {
                    return Err("duplicate named posting binding".into());
                }
            }
            PostingKey::Inferred(path) => {
                if field.custom_complete.is_some() || inferred.insert(path, index).is_some() {
                    return Err("inferred posting binding mismatch".into());
                }
            }
        }
    }
    if !structural.is_empty() {
        return Err("extra structural binding".into());
    }
    let mut raw = None;
    let mut fallback_text = None;
    let mut custom_text = HashMap::new();
    for binding in texts.bindings {
        if binding.alphabet != options.bigram_alphabet || binding.record_count != count {
            return Err("hybrid text binding alphabet or slot domain mismatch".into());
        }
        let allowed = if binding.key == TextKey::Fallback {
            fallback
        } else {
            logical
        };
        let mut index = BigramIndex::new(binding.alphabet);
        index.record_count = binding.record_count;
        if sealed {
            index.install_sealed(
                Arc::clone(&reader),
                BigramSpans {
                    normal: binding.normal.checked()?,
                    skip: binding.skip.checked()?,
                },
                Arc::clone(if binding.key == TextKey::Fallback {
                    fallback_shared.as_ref().unwrap()
                } else {
                    logical_shared.as_ref().unwrap()
                }),
            )?;
        } else {
            index.normal = materialize_dictionary::<u64>(
                &reader,
                SectionKind::BigramDictionaries,
                binding.normal,
                allowed,
            )?
            .into_iter()
            .collect();
            index.skip = materialize_dictionary::<u64>(
                &reader,
                SectionKind::BigramDictionaries,
                binding.skip,
                allowed,
            )?
            .into_iter()
            .collect();
        }
        match binding.key {
            TextKey::Raw => raw = Some(index),
            TextKey::Fallback => fallback_text = Some(index),
            TextKey::Named(path, name) => {
                if custom_text.insert((path, name), index).is_some() {
                    return Err("duplicate named text binding".into());
                }
            }
        }
    }
    let inference = match field_section.inference_report {
        Some(report) => {
            let mut empty_selections = HashMap::new();
            for (path, descriptor) in structure.inferred_empty_selections {
                if empty_selections
                    .insert(path, materialize(&reader, descriptor, logical)?)
                    .is_some()
                {
                    return Err("duplicate inferred empty selection".into());
                }
            }
            let mut short_string_coverage = HashMap::new();
            for (path, descriptor) in structure.short_string_coverage {
                if short_string_coverage
                    .insert(path, materialize(&reader, descriptor, logical)?)
                    .is_some()
                {
                    return Err("duplicate short-string coverage".into());
                }
            }
            Some(InferenceState {
                report,
                indexes: inferred,
                empty_selections,
                short_string_coverage,
            })
        }
        None if inferred.is_empty()
            && structure.inferred_empty_selections.is_empty()
            && structure.short_string_coverage.is_empty() =>
        {
            None
        }
        None => return Err("inference data without report".into()),
    };
    let sorting = decode_sort_data(
        required(&reader, SectionKind::SortData)?,
        &records,
        definition,
    )?;
    let relation_identity = decode_relation_identity(
        required(&reader, SectionKind::RelationAndIdentity)?,
        &records,
        definition,
    )?;
    let empty_array_records = materialize(&reader, structure.empty_array_records, logical)?;
    let segment = IndexedCollectionSegment {
        collection,
        source_id: records.source_id,
        source_version: records.source_version,
        files: records.files,
        file_index: records.file_index,
        records: records.records,
        id_lookup: records.id_lookup,
        identity_order: relation_identity.identity_order,
        postings: PostingCollection { fields: declared },
        inference,
        custom_postings: named,
        raw: raw.ok_or("raw text binding missing")?,
        fallback_text: fallback_text.ok_or("fallback text binding missing")?,
        custom_text,
        logical_records: records.logical_records,
        empty_array_records,
        sorting,
        relations: relation_identity.relations,
        record_parses: field_section.record_parses,
        failed_parses: field_section.failed_parses,
    };
    validate_components(&segment, definition, options)?;
    Ok(segment)
}

fn validate_components(
    segment: &IndexedCollectionSegment,
    definition: &CollectionDefinition,
    options: RepositoryOptions,
) -> Result<(), String> {
    let count = segment.records.len();
    if count > u32::MAX as usize
        || segment
            .logical_records
            .max()
            .is_some_and(|slot| slot as usize >= count)
        || !segment
            .empty_array_records
            .is_subset(&segment.logical_records)
    {
        return Err("hybrid record domain mismatch".into());
    }
    let logical_count = segment.logical_records.len() as usize;
    let expected_declared = definition
        .schema
        .fields
        .values()
        .filter(|field| {
            field.indexed
                || (!definition.schema.is_auto_index_prohibited(&field.path)
                    && (matches!(field.kind, LogicalType::Array(_))
                        || (field.kind == LogicalType::String && !field.multi)))
        })
        .map(|field| field.path.clone())
        .collect::<HashSet<_>>();
    if segment
        .postings
        .fields
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        != expected_declared
    {
        return Err("hybrid declared posting coverage mismatch".into());
    }
    for (path, index) in &segment.postings.fields {
        let field = &definition.schema.fields[path];
        if index.path != *path || index.kind != field.kind || index.multi != field.multi {
            return Err("hybrid declared posting binding mismatch".into());
        }
        validate_posting_stats(index)?;
    }
    segment.postings.validate_eligible(count, logical_count)?;
    segment.postings.validate_domain(&segment.logical_records)?;
    let expected_named = definition
        .schema
        .operations
        .iter()
        .filter(|(_, binding)| binding.operation.posting())
        .map(|(key, _)| key.clone())
        .collect::<HashSet<_>>();
    if segment
        .custom_postings
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        != expected_named
    {
        return Err("hybrid named posting coverage mismatch".into());
    }
    for (key, custom) in &segment.custom_postings {
        custom.index.validate_eligible(count, logical_count)?;
        custom.index.validate_domain(&segment.logical_records)?;
        validate_posting_stats(&custom.index)?;
        if custom.index.path != key.0
            || custom.index.kind != definition.schema.fields[&key.0].kind
            || (custom.complete
                && !definition.schema.operations[key]
                    .operation
                    .complete_coverage())
        {
            return Err("hybrid named posting binding mismatch".into());
        }
    }
    let mut fallback = RoaringBitmap::new();
    for record in &segment.records {
        if record.class == crate::inventory::RecordClass::RawFallback {
            fallback.insert(record.slot.0);
        }
    }
    for (index, allowed) in [
        (&segment.raw, &segment.logical_records),
        (&segment.fallback_text, &fallback),
    ] {
        index.validate(count)?;
        index.validate_domain(allowed)?;
        if index.alphabet != options.bigram_alphabet {
            return Err("hybrid text alphabet mismatch".into());
        }
    }
    let expected_text = definition
        .schema
        .custom
        .keys()
        .chain(
            definition
                .schema
                .operations
                .iter()
                .filter(|(_, binding)| {
                    binding.operation.bigram() && binding.operation.complete_coverage()
                })
                .map(|(key, _)| key),
        )
        .map(|key| canonical_custom_text_key(&definition.schema, &key.0, &key.1))
        .collect::<HashSet<_>>();
    if segment.custom_text.keys().cloned().collect::<HashSet<_>>() != expected_text {
        return Err("hybrid named text coverage mismatch".into());
    }
    for index in segment.custom_text.values() {
        index.validate(count)?;
        index.validate_domain(&segment.logical_records)?;
        if index.alphabet != options.bigram_alphabet {
            return Err("hybrid named text alphabet mismatch".into());
        }
    }
    match (&segment.inference, options.inference.mode) {
        (None, InferenceMode::Disabled) => {}
        (Some(state), InferenceMode::ReportOnly | InferenceMode::AutoIndex) => {
            state.validate(
                options.inference,
                &segment.logical_records,
                count,
                &definition.schema,
            )?;
            for index in state.indexes.values() {
                validate_posting_stats(index)?;
            }
        }
        _ => return Err("hybrid inference mode mismatch".into()),
    }
    Ok(())
}

fn validate_posting_stats(index: &PostingIndex) -> Result<(), String> {
    let valid = if let Some(values) = index.sealed_values() {
        values.matches_bounds(index.stats.min_key.as_ref(), index.stats.max_key.as_ref())?
    } else {
        index.stats.min_key.as_ref() == index.values.first_key_value().map(|(key, _)| key)
            && index.stats.max_key.as_ref() == index.values.last_key_value().map(|(key, _)| key)
    };
    if !valid {
        return Err("hybrid posting statistic bounds mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "hybrid_segment_codec.test.rs"]
mod tests;
