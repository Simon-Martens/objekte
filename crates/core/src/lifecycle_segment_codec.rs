//! Checked, source-I/O-free encoding of one indexed mutation segment.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use bincode::Options;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::bigram::BigramIndex;
use crate::build::{IndexedCollectionSegment, SegmentFile};
use crate::inference::{InferenceMode, InferenceState};
use crate::inventory::{
    FileEntry, FileStatus, MAX_DIAGNOSTIC_CODE_BYTES, MAX_DIAGNOSTIC_MESSAGE_BYTES,
    MAX_DIAGNOSTIC_PATH_SEGMENTS, MAX_DIAGNOSTIC_SEGMENT_BYTES, MAX_DIAGNOSTICS_PER_FILE,
    RecordClass, RecordEntry, SourceDiagnostic, SourceRevision,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::relation::RelationContribution;
use crate::repository::{
    CollectionDefinition, CollectionState, Repository, RepositoryOptions, canonical_custom_text_key,
};
use crate::sorting::{COLLATION_VERSION, SortCollection, compare_composite, simple_composite_part};
use crate::source::DiagnosticCategory;
use crate::storage::LifecycleSegmentCache;

const MAX_SEGMENT_BYTES: usize = 512 * 1024 * 1024;

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if !bytes.len().is_multiple_of(2) {
            return Err("invalid segment path encoding".into());
        }
        let wide = bytes
            .chunks_exact(2)
            .map(|part| u16::from_le_bytes([part[0], part[1]]))
            .collect::<Vec<_>>();
        Ok(std::ffi::OsString::from_wide(&wide).into())
    }
}


#[derive(Serialize, Deserialize)]
struct EncodedFile {
    id: FileId,
    path: Vec<u8>,
    revision: SourceRevision,
    created_ns: Option<i128>,
    collections: Vec<CollectionId>,
    status: FileStatus,
    diagnostics: Vec<SourceDiagnostic>,
    omitted_diagnostics: u32,
}


impl EncodedFile {
    fn from_entry(entry: &FileEntry) -> Self {
        Self {
            id: entry.id,
            path: path_bytes(&entry.relative_path),
            revision: entry.revision,
            created_ns: entry.created_ns,
            collections: entry.collections.clone(),
            status: entry.status,
            diagnostics: entry.diagnostics.clone(),
            omitted_diagnostics: entry.omitted_diagnostics,
        }
    }

    fn into_entry(self) -> Result<FileEntry, String> {
        Ok(FileEntry {
            id: self.id,
            relative_path: path_from_bytes(self.path)?,
            revision: self.revision,
            created_ns: self.created_ns,
            collections: self.collections,
            status: self.status,
            diagnostics: self.diagnostics,
            omitted_diagnostics: self.omitted_diagnostics,
        })
    }
}


#[derive(Serialize, Deserialize)]
struct EncodedSegmentFile {
    entry: EncodedFile,
    records: Vec<RecordSlot>,
}


#[derive(Serialize)]
struct SegmentRef<'a> {
    collection: CollectionId,
    source_id: &'a String,
    source_version: u32,
    files: &'a Vec<EncodedSegmentFile>,
    file_index: &'a HashMap<FileId, usize>,
    records: &'a Vec<RecordEntry>,
    id_lookup: &'a HashMap<RecordId, RecordSlot>,
    identity_order: &'a Vec<RecordSlot>,
    postings: &'a PostingCollection,
    inference: &'a Option<InferenceState>,
    custom_postings: &'a HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: &'a BigramIndex,
    fallback_text: &'a BigramIndex,
    custom_text: &'a HashMap<(Vec<String>, String), BigramIndex>,
    logical_records: &'a RoaringBitmap,
    empty_array_records: &'a RoaringBitmap,
    sorting: &'a SortCollection,
    relations: &'a Vec<RelationContribution>,
    record_parses: u64,
    failed_parses: u64,
}


#[derive(Deserialize)]
struct OwnedSegment {
    collection: CollectionId,
    source_id: String,
    source_version: u32,
    files: Vec<EncodedSegmentFile>,
    file_index: HashMap<FileId, usize>,
    records: Vec<RecordEntry>,
    id_lookup: HashMap<RecordId, RecordSlot>,
    identity_order: Vec<RecordSlot>,
    postings: PostingCollection,
    inference: Option<InferenceState>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramIndex,
    fallback_text: BigramIndex,
    custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortCollection,
    relations: Vec<RelationContribution>,
    record_parses: u64,
    failed_parses: u64,
}


/// Encode one completed segment. The outer snapshot section supplies the
/// checksum; this codec preserves native non-UTF-8 source paths as bytes.
pub(crate) fn encode_segment(segment: &IndexedCollectionSegment) -> Result<Vec<u8>, String> {
    let files = segment
        .files
        .iter()
        .map(|file| EncodedSegmentFile {
            entry: EncodedFile::from_entry(&file.entry),
            records: file.records.clone(),
        })
        .collect::<Vec<_>>();
    let value = SegmentRef {
        collection: segment.collection,
        source_id: &segment.source_id,
        source_version: segment.source_version,
        files: &files,
        file_index: &segment.file_index,
        records: &segment.records,
        id_lookup: &segment.id_lookup,
        identity_order: &segment.identity_order,
        postings: &segment.postings,
        inference: &segment.inference,
        custom_postings: &segment.custom_postings,
        raw: &segment.raw,
        fallback_text: &segment.fallback_text,
        custom_text: &segment.custom_text,
        logical_records: &segment.logical_records,
        empty_array_records: &segment.empty_array_records,
        sorting: &segment.sorting,
        relations: &segment.relations,
        record_parses: segment.record_parses,
        failed_parses: segment.failed_parses,
    };
    let mut bytes = Vec::new();
    codec()
        .serialize_into(&mut bytes, &value)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_SEGMENT_BYTES {
        return Err("segment snapshot exceeds byte limit".into());
    }
    Ok(bytes)
}

/// Decode and structurally validate a segment against the restored base and
/// catalog. `active` is the snapshot's local liveness mask for this segment.
pub(crate) fn decode_segment(
    bytes: &[u8],
    repository: &Repository,
    collection_id: CollectionId,
    catalog: &FileCatalog,
    active: &RoaringBitmap,
) -> Result<IndexedCollectionSegment, String> {
    if bytes.is_empty() || bytes.len() > MAX_SEGMENT_BYTES {
        return Err("segment snapshot byte length invalid".into());
    }
    let encoded: OwnedSegment = codec()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
        .map_err(|error| format!("invalid segment snapshot: {error}"))?;
    let files = encoded
        .files
        .into_iter()
        .map(|file| {
            Ok(SegmentFile {
                entry: file.entry.into_entry()?,
                records: file.records,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let segment = IndexedCollectionSegment {
        collection: encoded.collection,
        source_id: encoded.source_id,
        source_version: encoded.source_version,
        files,
        file_index: encoded.file_index,
        records: encoded.records,
        id_lookup: encoded.id_lookup,
        identity_order: encoded.identity_order,
        postings: encoded.postings,
        inference: encoded.inference,
        custom_postings: encoded.custom_postings,
        raw: encoded.raw,
        fallback_text: encoded.fallback_text,
        custom_text: encoded.custom_text,
        logical_records: encoded.logical_records,
        empty_array_records: encoded.empty_array_records,
        sorting: encoded.sorting,
        relations: encoded.relations,
        record_parses: encoded.record_parses,
        failed_parses: encoded.failed_parses,
    };
    let collection = repository
        .collections()
        .get(collection_id.0 as usize)
        .ok_or("segment collection missing")?;
    validate_segment(
        &segment,
        collection,
        repository.options(),
        catalog,
        active,
        repository.collections().len(),
    )?;
    Ok(segment)
}

/// Cross-check the outer cache section's local record count before returning
/// the decoded segment to the publication owner.
pub(crate) fn decode_cache_segment(
    cache: &LifecycleSegmentCache,
    repository: &Repository,
    catalog: &FileCatalog,
) -> Result<IndexedCollectionSegment, String> {
    let index = decode_segment(
        &cache.index,
        repository,
        cache.collection,
        catalog,
        &cache.active,
    )?;
    if index.records.len() != cache.record_count as usize {
        return Err("segment envelope record count mismatch".into());
    }
    Ok(index)
}

fn valid_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn validate_file_diagnostics(file: &FileEntry) -> Result<(), String> {
    if file.diagnostics.len() > MAX_DIAGNOSTICS_PER_FILE {
        return Err("segment file diagnostic table too large".into());
    }
    let derived = file
        .diagnostics
        .iter()
        .fold(FileStatus::Valid, |status, diagnostic| {
            status.max(match diagnostic.category {
                DiagnosticCategory::Io => FileStatus::Unreadable,
                _ => FileStatus::Broken,
            })
        });
    if (file.omitted_diagnostics == 0 && file.status != derived)
        || (file.omitted_diagnostics != 0
            && (file.status == FileStatus::Valid
                || derived == FileStatus::Valid
                || file.status < derived))
    {
        return Err("segment file status and diagnostics disagree".into());
    }
    for diagnostic in &file.diagnostics {
        if diagnostic.file != file.id
            || !file.collections.contains(&diagnostic.collection)
            || diagnostic.source_id.is_empty()
            || diagnostic.source_version == 0
            || diagnostic.code.is_empty()
            || diagnostic.code.len() > MAX_DIAGNOSTIC_CODE_BYTES
            || diagnostic.field_path.as_ref().is_some_and(|path| {
                path.len() > MAX_DIAGNOSTIC_PATH_SEGMENTS
                    || path
                        .iter()
                        .any(|part| part.len() > MAX_DIAGNOSTIC_SEGMENT_BYTES)
            })
            || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
            || diagnostic
                .range
                .is_some_and(|range| range.start > range.end || range.end > file.revision.size)
            || diagnostic
                .byte_offset
                .is_some_and(|offset| offset > file.revision.size)
        {
            return Err("segment source diagnostic invalid".into());
        }
    }
    Ok(())
}

fn validate_segment(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    options: RepositoryOptions,
    catalog: &FileCatalog,
    active: &RoaringBitmap,
    collection_count: usize,
) -> Result<(), String> {
    let definition = &collection.definition;
    let count = segment.records.len();
    if segment.collection != collection.generation.collection
        || segment.source_id != collection.source.id()
        || segment.source_version != collection.source.version()
        || count > u32::MAX as usize
        || active.max().is_some_and(|slot| slot as usize >= count)
    {
        return Err("segment collection, source, or active domain mismatch".into());
    }
    let mut expected_files = HashMap::<FileId, Vec<RecordSlot>>::new();
    let mut paths = HashSet::new();
    for (number, file) in segment.files.iter().enumerate() {
        validate_file_diagnostics(&file.entry)?;
        if !valid_path(&file.entry.relative_path)
            || file.entry.id.0 as usize >= catalog.allocated_files()
            || !paths.insert(file.entry.relative_path.clone())
            || !file.entry.collections.contains(&segment.collection)
            || file
                .entry
                .collections
                .iter()
                .any(|membership| membership.0 as usize >= collection_count)
            || file
                .entry
                .collections
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len()
                != file.entry.collections.len()
            || file.entry.diagnostics.iter().any(|diagnostic| {
                diagnostic.file != file.entry.id
                    || !file.entry.collections.contains(&diagnostic.collection)
            })
            || expected_files.insert(file.entry.id, Vec::new()).is_some()
            || segment.file_index.get(&file.entry.id) != Some(&number)
        {
            return Err("segment file identity or membership mismatch".into());
        }
    }
    if segment.file_index.len() != segment.files.len() {
        return Err("segment file lookup mismatch".into());
    }
    let mut ids = HashMap::new();
    let mut logical = RoaringBitmap::new();
    let mut fallback = RoaringBitmap::new();
    for (number, record) in segment.records.iter().enumerate() {
        let file = segment
            .file(record.file)
            .ok_or("segment record file missing")?;
        if record.slot.0 as usize != number
            || record.revision != file.entry.revision
            || record.range.start > record.range.end
            || record.range.end > file.entry.revision.size
            || ids.insert(record.id.clone(), record.slot).is_some()
        {
            return Err("segment record identity or source span invalid".into());
        }
        if let Some(index) = record.diagnostic_ref.stored_index() {
            let diagnostic = file
                .entry
                .diagnostics
                .get(index)
                .ok_or("segment record diagnostic out of bounds")?;
            if diagnostic.record != Some(record.slot) {
                return Err("segment record diagnostic ownership mismatch".into());
            }
        }
        expected_files
            .get_mut(&record.file)
            .ok_or("segment record file missing")?
            .push(record.slot);
        match record.class {
            RecordClass::Logical => {
                logical.insert(record.slot.0);
            }
            RecordClass::RawFallback => {
                fallback.insert(record.slot.0);
                let expected = RecordId::fallback(
                    segment.collection,
                    &segment.source_id,
                    segment.source_version,
                    &file.entry.relative_path,
                    record.file,
                    record.revision.modified_ns,
                    record.range.start,
                    record.range.end,
                );
                if record.id != expected {
                    return Err("segment fallback identity mismatch".into());
                }
            }
        }
        if active.contains(record.slot.0) {
            let current = catalog
                .entry(record.file)
                .ok_or("active segment source is retired")?;
            if current.relative_path != file.entry.relative_path
                || current.revision != file.entry.revision
                || current.created_ns != file.entry.created_ns
                || !current.collections.contains(&segment.collection)
            {
                return Err("active segment source metadata mismatch".into());
            }
        }
    }
    if segment.id_lookup != ids
        || segment.logical_records != logical
        || !segment.empty_array_records.is_subset(&logical)
    {
        return Err("segment identity or logical domain mismatch".into());
    }
    for file in &segment.files {
        if expected_files.get(&file.entry.id) != Some(&file.records) {
            return Err("segment file record mapping mismatch".into());
        }
    }
    let mut identity_order = segment
        .records
        .iter()
        .map(|record| record.slot)
        .collect::<Vec<_>>();
    identity_order.sort_by(|left, right| {
        segment.records[left.0 as usize]
            .id
            .cmp(&segment.records[right.0 as usize].id)
    });
    if segment.identity_order != identity_order {
        return Err("segment identity order mismatch".into());
    }
    let logical_count = logical.len() as usize;
    let expected_postings = definition
        .schema
        .fields
        .values()
        .filter(|field| field.indexed)
        .map(|field| field.path.clone())
        .collect::<HashSet<_>>();
    if segment
        .postings
        .fields
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        != expected_postings
    {
        return Err("segment posting coverage mismatch".into());
    }
    for (path, posting) in &segment.postings.fields {
        let field = &definition.schema.fields[path];
        if posting.path != *path || posting.kind != field.kind || posting.multi != field.multi {
            return Err("segment posting binding mismatch".into());
        }
    }
    segment.postings.validate_eligible(count, logical_count)?;
    if logical_count != count {
        segment.postings.validate_domain(&logical)?;
    }
    let expected_custom = definition
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
        != expected_custom
    {
        return Err("segment custom posting coverage mismatch".into());
    }
    for (key, custom) in &segment.custom_postings {
        custom.index.validate_eligible(count, logical_count)?;
        if logical_count != count {
            custom.index.validate_domain(&logical)?;
        }
        if custom.index.path != key.0
            || custom.index.kind != definition.schema.fields[&key.0].kind
            || (custom.complete
                && !definition.schema.operations[key]
                    .operation
                    .complete_coverage())
        {
            return Err("segment custom posting binding mismatch".into());
        }
    }
    for index in [&segment.raw, &segment.fallback_text] {
        index.validate(count)?;
        if index.alphabet != options.bigram_alphabet {
            return Err("segment bigram alphabet mismatch".into());
        }
    }
    segment.raw.validate_domain(&logical)?;
    segment.fallback_text.validate_domain(&fallback)?;
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
        return Err("segment custom bigram coverage mismatch".into());
    }
    for index in segment.custom_text.values() {
        index.validate(count)?;
        index.validate_domain(&logical)?;
        if index.alphabet != options.bigram_alphabet {
            return Err("segment custom bigram alphabet mismatch".into());
        }
    }
    match (&segment.inference, options.inference.mode) {
        (None, InferenceMode::Disabled) => {}
        (Some(inference), InferenceMode::ReportOnly | InferenceMode::AutoIndex) => {
            inference.validate(options.inference, &logical, count, &definition.schema)?;
        }
        _ => return Err("segment inference mode mismatch".into()),
    }
    validate_sorting(segment, definition)?;
    validate_relations(segment, collection, &logical, &fallback)?;
    Ok(())
}

fn valid_permutation(order: &[RecordSlot], count: usize) -> bool {
    if order.len() != count {
        return false;
    }
    let mut seen = vec![false; count];
    for slot in order {
        let index = slot.0 as usize;
        if index >= count || seen[index] {
            return false;
        }
        seen[index] = true;
    }
    true
}

fn validate_sorting(
    segment: &IndexedCollectionSegment,
    definition: &CollectionDefinition,
) -> Result<(), String> {
    let count = segment.records.len();
    if segment.sorting.columns.len() != definition.sort_fields.len() {
        return Err("segment sort coverage mismatch".into());
    }
    for path in &definition.sort_fields {
        let column = segment
            .sorting
            .columns
            .get(path)
            .ok_or("segment sort path missing")?;
        let expected_config = definition.sort_overrides.get(path);
        if column.path != *path
            || column.kind != definition.schema.field(path)?.kind
            || column.locale
                != expected_config.map_or(definition.sort_locale.as_str(), |config| {
                    config.locale.as_str()
                })
            || column.options
                != expected_config.map_or(definition.sort_options, |config| config.options)
            || column.collation_version != COLLATION_VERSION
            || column.keys.len() != count
            || !valid_permutation(&column.ascending, count)
            || !valid_permutation(&column.descending, count)
        {
            return Err("segment sort binding or permutation mismatch".into());
        }
        for (ordered, descending) in [(&column.ascending, false), (&column.descending, true)] {
            if ordered.windows(2).any(|pair| {
                let a = pair[0].0 as usize;
                let b = pair[1].0 as usize;
                column.keys[a]
                    .compare_with(&column.keys[b], descending, column.options)
                    .then_with(|| segment.records[a].id.cmp(&segment.records[b].id))
                    .is_gt()
            }) {
                return Err("segment sort order mismatch".into());
            }
        }
    }
    if segment.sorting.composites.len() != definition.schema.composite_sorts.len() {
        return Err("segment composite sort coverage mismatch".into());
    }
    for (number, expected) in definition.schema.composite_sorts.iter().enumerate() {
        let composite = &segment.sorting.composites[number];
        let normalized = expected
            .iter()
            .map(simple_composite_part)
            .collect::<Option<Vec<_>>>()
            .ok_or("invalid composite sort definition")?;
        if composite.fields != normalized || !valid_permutation(&composite.order, count) {
            return Err("segment composite sort definition mismatch".into());
        }
        let columns = normalized
            .iter()
            .map(|part| {
                segment
                    .sorting
                    .columns
                    .get(&part.path)
                    .map(|column| (column, part.descending))
                    .ok_or("segment composite sort column missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if composite.order.windows(2).any(|pair| {
            compare_composite(&columns, pair[0], pair[1])
                .then_with(|| {
                    segment.records[pair[0].0 as usize]
                        .id
                        .cmp(&segment.records[pair[1].0 as usize].id)
                })
                .is_gt()
        }) {
            return Err("segment composite sort order mismatch".into());
        }
    }
    Ok(())
}

fn validate_relations(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    logical: &RoaringBitmap,
    fallback: &RoaringBitmap,
) -> Result<(), String> {
    let declared = collection
        .definition
        .schema
        .relations()
        .values()
        .collect::<Vec<_>>();
    if segment.relations.len() != declared.len() {
        return Err("segment relation coverage mismatch".into());
    }
    for (relation, expected) in segment.relations.iter().zip(declared) {
        if relation.declaration() != expected
            || relation.record_count() as usize != segment.records.len()
            || relation.unavailable() != fallback
            || !relation.present().is_subset(logical)
            || !relation.nulls().is_subset(relation.present())
            || !relation.empty_arrays().is_subset(relation.present())
            || segment
                .postings
                .fields
                .get(&expected.path)
                .is_none_or(|posting| &posting.presence != relation.present())
        {
            return Err("segment relation identity or source domain mismatch".into());
        }
        let mut previous = None;
        for (source, _target, _positions) in relation.references() {
            if source.0 as usize >= segment.records.len()
                || !logical.contains(source.0)
                || !relation.present().contains(source.0)
                || previous.is_some_and(|last| source.0 < last)
            {
                return Err("segment relation reference source invalid".into());
            }
            previous = Some(source.0);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_segment_codec.test.rs"]
mod tests;
