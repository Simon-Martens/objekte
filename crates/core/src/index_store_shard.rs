//! Writing and reading one shard in the redb index store.

use std::collections::{BTreeMap, HashMap};

use redb::{ReadTransaction, TableDefinition, WriteTransaction};
use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::build::{BaseSegmentParts, IndexedCollectionSegment, ShardRole};
use crate::index_store_keys as keys;
use crate::inference::InferenceState;
use crate::inventory::{ActiveRecords, RecordEntry};
use crate::model::{Number, RecordId, RecordSlot};
use crate::posting::{PostingIndex, PostingParts, StructuralFacts, StructuralKey};
use crate::relation::{PendingReference, RelationContribution};
use crate::slot_store::SlotTable;
use crate::sorting::{
    CompositeSortOrder, SortCollection, SortColumn, SortColumnParts, SortKey, compare_composite,
};

type Table<'a> = redb::Table<'a, &'static [u8], &'static [u8]>;

fn error(error: impl std::fmt::Display) -> String {
    format!("index store: {error}")
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    bincode::serialize(value).map_err(error)
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    bincode::deserialize(bytes).map_err(error)
}

fn bitmap_bytes(bitmap: &RoaringBitmap) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(bitmap.serialized_size());
    bitmap.serialize_into(&mut bytes).map_err(error)?;
    Ok(bytes)
}

fn read_bitmap(bytes: &[u8]) -> Result<RoaringBitmap, String> {
    RoaringBitmap::deserialize_from(bytes).map_err(error)
}

fn field_bytes(path: &[String]) -> Result<Vec<u8>, String> {
    encode(&path)
}

fn custom_field_bytes(path: &[String], binding: &str) -> Result<Vec<u8>, String> {
    encode(&(path, binding))
}

fn structural_code(key: StructuralKey) -> u8 {
    match key {
        StructuralKey::AnyNull => 0,
        StructuralKey::NonEmpty => 1,
        StructuralKey::String => 2,
        StructuralKey::Number => 3,
        StructuralKey::Array => 4,
        StructuralKey::Object => 5,
        StructuralKey::EmptyObject => 6,
    }
}

fn structural_key(code: u8) -> Option<StructuralKey> {
    Some(match code {
        0 => StructuralKey::AnyNull,
        1 => StructuralKey::NonEmpty,
        2 => StructuralKey::String,
        3 => StructuralKey::Number,
        4 => StructuralKey::Array,
        5 => StructuralKey::Object,
        6 => StructuralKey::EmptyObject,
        _ => return None,
    })
}

type RawPosting = (
    BTreeMap<crate::posting::ScalarKey, RoaringBitmap>,
    BTreeMap<Number, RoaringBitmap>,
    BTreeMap<StructuralKey, RoaringBitmap>,
);

fn write_bitmap_chunks(
    postings: &mut Table<'_>,
    prefix: &[u8],
    family: u8,
    field: &[u8],
    key_bytes: &[u8],
    posting: &RoaringBitmap,
) -> Result<(), String> {
    let mut chunks = crate::posting_map::split_chunks(posting.clone());
    if chunks.is_empty() {
        chunks.push((0, RoaringBitmap::new()));
    }
    for (chunk, part) in chunks {
        postings
            .insert(
                keys::posting_key(prefix, family, field, key_bytes, chunk).as_slice(),
                bitmap_bytes(&part)?.as_slice(),
            )
            .map_err(error)?;
    }
    Ok(())
}

struct PostingWrite<'a> {
    prefix: &'a [u8],
    family: u8,
    bitmap_offset: u8,
    field: &'a [u8],
    custom_complete: Option<bool>,
}

fn write_posting(
    fields: &mut Table<'_>,
    postings: &mut Table<'_>,
    bitmaps: &mut Table<'_>,
    write: PostingWrite<'_>,
    index: &PostingIndex,
) -> Result<(), String> {
    let PostingWrite {
        prefix,
        family,
        bitmap_offset,
        field,
        custom_complete,
    } = write;
    fields
        .insert(
            keys::posting_field_key(prefix, family, field).as_slice(),
            encode(&keys::PostingHeader {
                kind: index.kind.clone(),
                multi: index.multi,
                values_indexed: index.values_indexed,
                value_complete: index.value_complete,
                length_complete: index.length_complete,
                empty_string_complete: index.empty_string_complete,
                covered_records: index.covered_records,
                has_structural: index.structural.is_some(),
                custom_complete,
            })?
            .as_slice(),
        )
        .map_err(error)?;
    index.for_each_value(|key, slots| {
        let key_bytes = keys::encode_scalar_key(key);
        write_bitmap_chunks(
            postings,
            prefix,
            family,
            field,
            &key_bytes,
            slots,
        )
    })?;
    index.for_each_length(|key, slots| {
        let key_bytes = keys::encode_number_value(key);
        write_bitmap_chunks(
            postings,
            prefix,
            family + 1,
            field,
            &key_bytes,
            slots,
        )
    })?;
    if let Some(facts) = &index.structural {
        facts.for_each(|key, slots| {
            let key_bytes = [structural_code(key)];
            write_bitmap_chunks(
                postings,
                prefix,
                family + 2,
                field,
                &key_bytes,
                slots,
            )
        })?;
    }
    for (kind, bitmap) in [
        (keys::BITMAP_PRESENCE, &index.presence),
        (keys::BITMAP_NULLS, &index.nulls),
        (keys::BITMAP_EMPTY_SELECTIONS, &index.empty_selections),
    ] {
        bitmaps
            .insert(
                keys::bitmap_key(prefix, kind + bitmap_offset, field).as_slice(),
                bitmap_bytes(bitmap)?.as_slice(),
            )
            .map_err(error)?;
    }
    Ok(())
}

fn write_gram(
    postings: &mut Table<'_>,
    prefix: &[u8],
    base: u8,
    field: &[u8],
    index: &BigramIndex,
) -> Result<(), String> {
    for (skip, family_base) in [(false, base), (true, base + 1)] {
        let entries = if skip {
            index.skip_entries()
        } else {
            index.normal_entries()
        };
        for (key, posting) in entries {
            let mut chunks = crate::posting_map::split_chunks(posting);
            if chunks.is_empty() {
                chunks.push((0, RoaringBitmap::new()));
            }
            for (chunk, part) in chunks {
                postings
                    .insert(
                        keys::posting_key(prefix, family_base, field, &key.to_be_bytes(), chunk)
                            .as_slice(),
                        bitmap_bytes(&part)?.as_slice(),
                    )
                    .map_err(error)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn write_shard(
    write: &WriteTransaction,
    collection: u32,
    store_id: u64,
    ordinal: u64,
    shard: &IndexedCollectionSegment,
    active: &ActiveRecords,
) -> Result<(), String> {
    let prefix = keys::prefix(collection, store_id);
    let mut table = write.open_table(keys::SHARDS).map_err(error)?;
    table
        .insert(
            prefix.as_slice(),
            encode(&keys::ShardHeader {
                ordinal,
                alphabet: shard.raw.alphabet,
                role: match shard.role {
                    ShardRole::Built => 0,
                    ShardRole::Mutation => 1,
                    ShardRole::Main => 2,
                },
                shared: shard.identity_order().is_shared(),
                source_id: shard.source_id.clone(),
                source_version: shard.source_version,
                record_count: shard.record_count() as u32,
                file_count: shard.file_count() as u32,
                dense_epoch: shard.dense_epoch,
                record_parses: shard.record_parses,
                failed_parses: shard.failed_parses,
                has_inference: shard.inference.is_some(),
            })?
            .as_slice(),
        )
        .map_err(error)?;
    drop(table);

    let mut table = write.open_table(keys::ACTIVE).map_err(error)?;
    table
        .insert(prefix.as_slice(), bitmap_bytes(&active.bitmap())?.as_slice())
        .map_err(error)?;
    drop(table);

    let mut table = write.open_table(keys::FILES).map_err(error)?;
    for (position, file) in shard.files_iter().enumerate() {
        table
            .insert(
                keys::files_key(&prefix, position as u32).as_slice(),
                encode(file)?.as_slice(),
            )
            .map_err(error)?;
    }
    drop(table);

    let mut table = write.open_table(keys::RECORDS).map_err(error)?;
    for record in shard.records_iter() {
        table
            .insert(
                keys::records_key(&prefix, record.slot.0).as_slice(),
                encode(record)?.as_slice(),
            )
            .map_err(error)?;
    }
    drop(table);

    let mut fields = write.open_table(keys::POSTING_FIELDS).map_err(error)?;
    let mut postings = write.open_table(keys::POSTINGS).map_err(error)?;
    let mut bitmaps = write.open_table(keys::BITMAPS).map_err(error)?;
    for (path, index) in &shard.postings.fields {
        write_posting(
            &mut fields,
            &mut postings,
            &mut bitmaps,
            PostingWrite {
                prefix: &prefix,
                family: keys::FAMILY_DECLARED_VALUES,
                bitmap_offset: 0,
                field: &field_bytes(path)?,
                custom_complete: None,
            },
            index,
        )?;
    }
    for ((path, binding), custom) in &shard.custom_postings {
        let field = custom_field_bytes(path, binding)?;
        write_posting(
            &mut fields,
            &mut postings,
            &mut bitmaps,
            PostingWrite {
                prefix: &prefix,
                family: keys::FAMILY_CUSTOM_VALUES,
                bitmap_offset: 10,
                field: field.as_slice(),
                custom_complete: Some(custom.complete),
            },
            &custom.index,
        )?;
    }
    if let Some(inference) = &shard.inference {
        for (path, index) in &inference.indexes {
            let field = field_bytes(path)?;
            write_posting(
                &mut fields,
                &mut postings,
                &mut bitmaps,
                PostingWrite {
                    prefix: &prefix,
                    family: keys::FAMILY_INFERRED_VALUES,
                    bitmap_offset: 20,
                    field: field.as_slice(),
                    custom_complete: None,
                },
                index,
            )?;
        }
    }
    drop(fields);

    write_gram(
        &mut postings,
        &prefix,
        keys::FAMILY_RAW_NORMAL,
        b"",
        &shard.raw,
    )?;
    write_gram(
        &mut postings,
        &prefix,
        keys::FAMILY_FALLBACK_NORMAL,
        b"",
        &shard.fallback_text,
    )?;
    for ((path, binding), index) in &shard.custom_text {
        write_gram(
            &mut postings,
            &prefix,
            keys::FAMILY_CUSTOM_TEXT_NORMAL,
            &custom_field_bytes(path, binding)?,
            index,
        )?;
    }
    drop(postings);

    for (kind, bitmap) in [
        (keys::BITMAP_LOGICAL_RECORDS, shard.logical_records()),
        (keys::BITMAP_EMPTY_ARRAY_RECORDS, shard.empty_array_records()),
        (keys::BITMAP_FREE_SLOTS, &shard.free_slots),
        (keys::BITMAP_FREE_FILE_ROWS, &shard.free_file_rows),
    ] {
        bitmaps
            .insert(
                keys::bitmap_key(&prefix, kind, b"").as_slice(),
                bitmap_bytes(bitmap)?.as_slice(),
            )
            .map_err(error)?;
    }
    if let Some(inference) = &shard.inference {
        for (kind, map) in [
            (
                keys::BITMAP_INFERRED_EMPTY_SELECTIONS,
                &inference.empty_selections,
            ),
            (
                keys::BITMAP_INFERRED_SHORT_STRING_COVERAGE,
                &inference.short_string_coverage,
            ),
        ] {
            for (path, bitmap) in map {
                bitmaps
                    .insert(
                        keys::bitmap_key(&prefix, kind, &field_bytes(path)?).as_slice(),
                        bitmap_bytes(bitmap)?.as_slice(),
                    )
                    .map_err(error)?;
            }
        }
    }
    for (ordinal, relation) in shard.relations().iter().enumerate() {
        for (kind, bitmap) in [
            (keys::BITMAP_RELATION_PRESENT, &relation.present),
            (keys::BITMAP_RELATION_NULLS, &relation.nulls),
            (keys::BITMAP_RELATION_EMPTY_ARRAYS, &relation.empty_arrays),
            (keys::BITMAP_RELATION_UNAVAILABLE, &relation.unavailable),
        ] {
            bitmaps
                .insert(
                    keys::bitmap_key(&prefix, kind, &(ordinal as u32).to_be_bytes()).as_slice(),
                    bitmap_bytes(bitmap)?.as_slice(),
                )
                .map_err(error)?;
        }
    }
    drop(bitmaps);

    let mut columns = write.open_table(keys::SORT_COLUMNS).map_err(error)?;
    let mut sort_keys = write.open_table(keys::SORT_KEYS).map_err(error)?;
    for column in shard.sorting.columns.values() {
        let column_bytes = encode(&column.path)?;
        columns
            .insert(
                keys::sort_column_key(&prefix, &column_bytes).as_slice(),
                encode(&keys::SortColumnHeader {
                    path: column.path.clone(),
                    kind: column.kind.clone(),
                    locale: column.locale.clone(),
                    options: column.options,
                    collation_version: column.collation_version.clone(),
                })?
                .as_slice(),
            )
            .map_err(error)?;
        for (slot, key) in column.keys_iter().enumerate() {
            sort_keys
                .insert(
                    keys::sort_key_key(&prefix, &column_bytes, slot as u32).as_slice(),
                    keys::encode_sort_key(key).as_slice(),
                )
                .map_err(error)?;
        }
    }
    drop(columns);
    drop(sort_keys);

    let mut composites = write.open_table(keys::COMPOSITES).map_err(error)?;
    for (index, composite) in shard.sorting.composites.iter().enumerate() {
        composites
            .insert(
                keys::composite_key(&prefix, index as u32).as_slice(),
                encode(&composite.fields)?.as_slice(),
            )
            .map_err(error)?;
    }
    drop(composites);

    let mut relations = write.open_table(keys::RELATIONS).map_err(error)?;
    let mut references = write.open_table(keys::REFERENCES).map_err(error)?;
    for (ordinal, relation) in shard.relations().iter().enumerate() {
        relations
            .insert(
                keys::relation_key(&prefix, ordinal as u32).as_slice(),
                encode(&keys::RelationHeader {
                    declaration: relation.declaration.clone(),
                    record_count: relation.record_count,
                    reference_count: relation.reference_rows().count() as u32,
                })?
                .as_slice(),
            )
            .map_err(error)?;
        for (index, (source, target, positions)) in relation.reference_rows().enumerate() {
            references
                .insert(
                    keys::reference_key(&prefix, ordinal as u32, source.0, index as u32).as_slice(),
                    encode(&PendingReference {
                        source,
                        target_id: target.to_owned(),
                        positions: positions.to_vec(),
                    })?
                    .as_slice(),
                )
                .map_err(error)?;
        }
    }
    drop(relations);
    drop(references);

    if let Some(inference) = &shard.inference {
        let mut table = write.open_table(keys::INFERENCE).map_err(error)?;
        table
            .insert(prefix.as_slice(), encode(&inference.report)?.as_slice())
            .map_err(error)?;
    }
    Ok(())
}

type StoredRows = Vec<(Vec<u8>, Vec<u8>)>;

fn rows(
    read: &ReadTransaction,
    definition: TableDefinition<&[u8], &[u8]>,
    prefix: &[u8],
) -> Result<StoredRows, String> {
    let table = read.open_table(definition).map_err(error)?;
    let end = keys::prefix_end(prefix).ok_or("shard prefix overflow")?;
    let mut rows = Vec::new();
    for row in table
        .range::<&[u8]>(prefix..end.as_slice())
        .map_err(error)?
    {
        let (key, value) = row.map_err(error)?;
        rows.push((key.value().to_vec(), value.value().to_vec()));
    }
    Ok(rows)
}

fn split_bytes<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    keys::read_bytes(bytes, offset)
}

fn field_path(bytes: &[u8]) -> Result<Vec<String>, String> {
    decode(bytes)
}

fn bitmap_map(
    rows: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<HashMap<(u8, Vec<u8>), RoaringBitmap>, String> {
    let mut result = HashMap::new();
    for (key, value) in rows {
        let (kind, field) = keys::bitmap_parts(&key).ok_or("invalid bitmap key")?;
        result.insert((kind, field.to_vec()), read_bitmap(&value)?);
    }
    Ok(result)
}

fn build_index(
    header: &keys::PostingHeader,
    path: Vec<String>,
    presence: RoaringBitmap,
    nulls: RoaringBitmap,
    empty_selections: RoaringBitmap,
    raw: RawPosting,
) -> PostingIndex {
    let mut index = PostingIndex::from_parts(PostingParts {
        path,
        kind: header.kind.clone(),
        multi: header.multi,
        presence,
        empty_selections,
        nulls,
        values: raw.0,
        lengths: raw.1,
        value_complete: header.value_complete,
        length_complete: header.length_complete,
        empty_string_complete: header.empty_string_complete,
        covered_records: header.covered_records,
    });
    index.values_indexed = header.values_indexed;
    if header.has_structural {
        let mut facts = StructuralFacts::default();
        for (key, posting) in raw.2 {
            facts.set(key, posting);
        }
        index.structural = Some(facts);
    } else {
        index.structural = None;
    }
    index.finish_stats();
    index
}

pub(crate) fn read_shard(
    read: &ReadTransaction,
    collection: u32,
    store_id: u64,
) -> Result<(IndexedCollectionSegment, ActiveRecords, keys::ShardHeader), String> {
    let prefix = keys::prefix(collection, store_id);
    let header_rows = rows(read, keys::SHARDS, &prefix)?;
    let header: keys::ShardHeader = decode(
        header_rows
            .first()
            .ok_or("shard header missing")?
            .1
            .as_slice(),
    )?;
    let record_count = header.record_count as usize;

    let mut files = Vec::new();
    for (_, value) in rows(read, keys::FILES, &prefix)? {
        files.push(decode::<crate::build::SegmentFile>(&value)?);
    }
    let mut records = Vec::new();
    for (_, value) in rows(read, keys::RECORDS, &prefix)? {
        records.push(decode::<RecordEntry>(&value)?);
    }
    let active = rows(read, keys::ACTIVE, &prefix)?
        .first()
        .map(|(_, value)| read_bitmap(value))
        .transpose()?
        .unwrap_or_default();
    let active = ActiveRecords::from_bitmap(record_count, active)?;

    let mut field_headers: HashMap<(u8, Vec<u8>), keys::PostingHeader> = HashMap::new();
    for (key, value) in rows(read, keys::POSTING_FIELDS, &prefix)? {
        let mut offset = keys::SHARD_PREFIX_LEN;
        let family = *key.get(offset).ok_or("invalid posting field key")?;
        offset += 1;
        let field = split_bytes(&key, &mut offset)
            .ok_or("invalid posting field key")?
            .to_vec();
        field_headers.insert((family, field), decode(&value)?);
    }
    let mut raw_postings: HashMap<(u8, Vec<u8>), RawPosting> = HashMap::new();
    for (key, value) in rows(read, keys::POSTINGS, &prefix)? {
        let (family, field_bytes, posting_key, _chunk) =
            keys::posting_parts(&key).ok_or("invalid posting key")?;
        let entry = raw_postings
            .entry((family, field_bytes.to_vec()))
            .or_default();
        let bitmap = read_bitmap(&value)?;
        match family {
            0 | 3 | 6 => {
                let key = keys::decode_scalar_key(posting_key).ok_or("invalid scalar posting key")?;
                *entry.0.entry(key).or_default() |= bitmap;
            }
            1 | 4 | 7 => {
                let key = keys::decode_number_value(posting_key).ok_or("invalid length key")?;
                *entry.1.entry(key).or_default() |= bitmap;
            }
            2 | 5 | 8 => {
                let key = structural_key(*posting_key.first().ok_or("invalid structural key")?)
                    .ok_or("invalid structural key")?;
                *entry.2.entry(key).or_default() |= bitmap;
            }
            _ => {}
        }
    }
    let bitmaps = bitmap_map(rows(read, keys::BITMAPS, &prefix)?)?;
    let shard_free_slots = bitmaps
        .get(&(keys::BITMAP_FREE_SLOTS, Vec::new()))
        .cloned()
        .unwrap_or_default();

    let mut postings = crate::posting::PostingCollection {
        fields: HashMap::new(),
    };
    for ((family, field), header) in &field_headers {
        if *family != 0 {
            continue;
        }
        let bitmap_offset = match family {
            0 => 0,
            3 => 10,
            _ => 20,
        };
        let mut raw = raw_postings
            .remove(&(*family, field.clone()))
            .unwrap_or_default();
        if let Some(lengths) = raw_postings.remove(&(family + 1, field.clone())) {
            raw.1.extend(lengths.1);
        }
        if let Some(structural) = raw_postings.remove(&(family + 2, field.clone())) {
            raw.2.extend(structural.2);
        }
        let presence = bitmaps
            .get(&(keys::BITMAP_PRESENCE + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        let nulls = bitmaps
            .get(&(keys::BITMAP_NULLS + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        let empty = bitmaps
            .get(&(keys::BITMAP_EMPTY_SELECTIONS + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        let index = build_index(
            header,
            field_path(field)?,
            presence,
            nulls,
            empty,
            raw,
        );
        postings.fields.insert(field_path(field)?, index);
    }
    // Custom and inferred families are assembled separately below; the loop
    // above only installed declared fields. Reconstruct them from headers.
    let mut custom: HashMap<(Vec<String>, String), crate::posting::CustomPostingIndex> =
        HashMap::new();
    let mut inferred: HashMap<Vec<String>, PostingIndex> = HashMap::new();
    for ((family, field), header) in &field_headers {
        if !matches!(family, 3 | 6) {
            continue;
        }
        let mut raw = raw_postings
            .remove(&(*family, field.clone()))
            .unwrap_or_default();
        if let Some(lengths) = raw_postings.remove(&(family + 1, field.clone())) {
            raw.1.extend(lengths.1);
        }
        if let Some(structural) = raw_postings.remove(&(family + 2, field.clone())) {
            raw.2.extend(structural.2);
        }
        let bitmap_offset = if *family == 3 { 10 } else { 20 };
        let presence = bitmaps
            .get(&(keys::BITMAP_PRESENCE + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        let nulls = bitmaps
            .get(&(keys::BITMAP_NULLS + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        let empty = bitmaps
            .get(&(keys::BITMAP_EMPTY_SELECTIONS + bitmap_offset, field.clone()))
            .cloned()
            .unwrap_or_default();
        if *family == 3 {
            let (path, binding) = decode::<(Vec<String>, String)>(field)?;
            let index = build_index(header, path.clone(), presence, nulls, empty, raw);
            custom.insert(
                (path, binding),
                crate::posting::CustomPostingIndex {
                    index,
                    complete: header.custom_complete.unwrap_or(false),
                },
            );
        } else {
            let path = field_path(field)?;
            let index = build_index(header, path.clone(), presence, nulls, empty, raw);
            inferred.insert(path, index);
        }
    }
    let _ = &mut postings;

    // Bigram families.
    let mut normal: BTreeMap<u64, RoaringBitmap> = BTreeMap::new();
    let mut skip: BTreeMap<u64, RoaringBitmap> = BTreeMap::new();
    let mut fallback_normal = BTreeMap::new();
    let mut fallback_skip = BTreeMap::new();
    type GramParts = (BTreeMap<u64, RoaringBitmap>, BTreeMap<u64, RoaringBitmap>);
    let mut custom_text_parts: HashMap<(Vec<String>, String), GramParts> = HashMap::new();
    for (key, value) in rows(read, keys::POSTINGS, &prefix)? {
        let (family, field, posting_key, _) =
            keys::posting_parts(&key).ok_or("invalid gram key")?;
        if !matches!(family, 9..=14) {
            continue;
        }
        let gram = u64::from_be_bytes(
            posting_key
                .try_into()
                .map_err(|_| "invalid gram key")?,
        );
        let bitmap = read_bitmap(&value)?;
        if family >= 13 {
            let key = decode::<(Vec<String>, String)>(field)?;
            let entry = custom_text_parts.entry(key).or_default();
            let target = if family == 13 {
                &mut entry.0
            } else {
                &mut entry.1
            };
            *target.entry(gram).or_default() |= bitmap;
        } else {
            let target = match family {
                9 => &mut normal,
                10 => &mut skip,
                11 => &mut fallback_normal,
                _ => &mut fallback_skip,
            };
            *target.entry(gram).or_default() |= bitmap;
        }
    }
    let mut custom_text = HashMap::new();
    for (key, (normal, skip)) in custom_text_parts {
        custom_text.insert(
            key,
            BigramIndex::from_parts(header.alphabet, header.record_count, normal, skip),
        );
    }
    let raw_bigram = BigramIndex::from_parts(
        header.alphabet,
        header.record_count,
        normal,
        skip,
    );
    let fallback_bigram = BigramIndex::from_parts(
        header.alphabet,
        header.record_count,
        fallback_normal,
        fallback_skip,
    );

    // Sort columns and orders.
    let mut headers = Vec::new();
    for (key, value) in rows(read, keys::SORT_COLUMNS, &prefix)? {
        let mut offset = keys::SHARD_PREFIX_LEN;
        let column = split_bytes(&key, &mut offset)
            .ok_or("invalid sort column key")?
            .to_vec();
        headers.push((column, decode::<keys::SortColumnHeader>(&value)?));
    }
    let mut key_rows: HashMap<Vec<u8>, Vec<(u32, SortKey)>> = HashMap::new();
    for (key, value) in rows(read, keys::SORT_KEYS, &prefix)? {
        let mut offset = keys::SHARD_PREFIX_LEN;
        let column = split_bytes(&key, &mut offset)
            .ok_or("invalid sort key key")?
            .to_vec();
        let slot = keys::read_u32(&key, &mut offset).ok_or("invalid sort key key")?;
        key_rows
            .entry(column)
            .or_default()
            .push((slot, keys::decode_sort_key(&value).ok_or("invalid sort key")?));
    }
    let mut sorting = SortCollection {
        columns: HashMap::new(),
        composites: Vec::new(),
    };
    for (column, header) in headers {
        let mut keys = vec![SortKey::Missing; record_count];
        if let Some(rows) = key_rows.get(&column) {
            for (slot, key) in rows {
                keys[*slot as usize] = key.clone();
            }
        }
        let mut sort_column = SortColumn::from_parts(SortColumnParts {
            path: header.path.clone(),
            kind: header.kind,
            keys,
            ascending: Vec::new(),
            descending: Vec::new(),
            locale: header.locale,
            options: header.options,
            collation_version: header.collation_version,
        });
        let mut ascending = (0..record_count)
            .filter(|slot| !shard_free_slots.contains(*slot as u32))
            .map(|slot| RecordSlot(slot as u32))
            .collect::<Vec<_>>();
        ascending.sort_by(|a, b| {
            sort_column
                .key(*a)
                .unwrap()
                .compare_with(sort_column.key(*b).unwrap(), false, sort_column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
                .then_with(|| a.cmp(b))
        });
        let mut descending = (0..record_count)
            .filter(|slot| !shard_free_slots.contains(*slot as u32))
            .map(|slot| RecordSlot(slot as u32))
            .collect::<Vec<_>>();
        descending.sort_by(|a, b| {
            sort_column
                .key(*a)
                .unwrap()
                .compare_with(sort_column.key(*b).unwrap(), true, sort_column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
                .then_with(|| a.cmp(b))
        });
        sort_column.set_orders(ascending, descending);
        sorting.columns.insert(header.path, sort_column);
    }
    let mut composite_rows = rows(read, keys::COMPOSITES, &prefix)?;
    composite_rows.sort_by_key(|(key, _)| key.clone());
    for (_, value) in composite_rows {
        let fields = decode::<Vec<crate::sorting::CompositeSortPart>>(&value)?;
        let columns = fields
            .iter()
            .map(|field| (&sorting.columns[&field.path], field.descending))
            .collect::<Vec<_>>();
        let mut order = (0..record_count)
            .filter(|slot| !shard_free_slots.contains(*slot as u32))
            .map(|slot| RecordSlot(slot as u32))
            .collect::<Vec<_>>();
        order.sort_by(|a, b| {
            compare_composite(&columns, *a, *b)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
                .then_with(|| a.cmp(b))
        });
        sorting.composites.push(CompositeSortOrder::from_parts(fields, order));
    }

    // Relations.
    let mut relation_headers = Vec::new();
    for (key, value) in rows(read, keys::RELATIONS, &prefix)? {
        let mut offset = keys::SHARD_PREFIX_LEN;
        let ordinal = keys::read_u32(&key, &mut offset).ok_or("invalid relation key")?;
        relation_headers.push((ordinal, decode::<keys::RelationHeader>(&value)?));
    }
    let mut relation_refs: HashMap<u32, Vec<PendingReference>> = HashMap::new();
    for (key, value) in rows(read, keys::REFERENCES, &prefix)? {
        let mut offset = keys::SHARD_PREFIX_LEN;
        let ordinal = keys::read_u32(&key, &mut offset).ok_or("invalid reference key")?;
        relation_refs
            .entry(ordinal)
            .or_default()
            .push(decode(&value)?);
    }
    relation_headers.sort_by_key(|(ordinal, _)| *ordinal);
    let mut relations = Vec::new();
    for (ordinal, header) in relation_headers {
        let mut references = relation_refs.remove(&ordinal).unwrap_or_default();
        references.sort_by_key(|reference| reference.source);
        relations.push(RelationContribution {
            declaration: header.declaration,
            record_count: header.record_count,
            references: SlotTable::from_vec(references),
            present: bitmaps
                .get(&(
                    keys::BITMAP_RELATION_PRESENT,
                    ordinal.to_be_bytes().to_vec(),
                ))
                .cloned()
                .unwrap_or_default(),
            nulls: bitmaps
                .get(&(keys::BITMAP_RELATION_NULLS, ordinal.to_be_bytes().to_vec()))
                .cloned()
                .unwrap_or_default(),
            empty_arrays: bitmaps
                .get(&(
                    keys::BITMAP_RELATION_EMPTY_ARRAYS,
                    ordinal.to_be_bytes().to_vec(),
                ))
                .cloned()
                .unwrap_or_default(),
            unavailable: bitmaps
                .get(&(
                    keys::BITMAP_RELATION_UNAVAILABLE,
                    ordinal.to_be_bytes().to_vec(),
                ))
                .cloned()
                .unwrap_or_default(),
        });
    }

    // Inference.
    let inference = if header.has_inference {
        let report = rows(read, keys::INFERENCE, &prefix)?
            .first()
            .map(|(_, value)| decode(value))
            .transpose()?
            .ok_or("inference report missing")?;
        let mut empty_selections = HashMap::new();
        let mut short_string_coverage = HashMap::new();
        for ((kind, field), bitmap) in &bitmaps {
            if *kind == keys::BITMAP_INFERRED_EMPTY_SELECTIONS {
                empty_selections.insert(field_path(field)?, bitmap.clone());
            } else if *kind == keys::BITMAP_INFERRED_SHORT_STRING_COVERAGE {
                short_string_coverage.insert(field_path(field)?, bitmap.clone());
            }
        }
        Some(InferenceState {
            report,
            indexes: inferred,
            empty_selections,
            short_string_coverage,
        })
    } else {
        None
    };

    let mut file_index = HashMap::new();
    for (position, file) in files.iter().enumerate() {
        file_index.insert(file.entry.id, position);
    }
    let mut id_lookup: HashMap<RecordId, RecordSlot> = HashMap::new();
    for record in &records {
        if shard_free_slots.contains(record.slot.0) {
            continue;
        }
        match id_lookup.get(&record.id) {
            Some(existing) if active.contains(*existing) => {}
            _ => {
                id_lookup.insert(record.id.clone(), record.slot);
            }
        }
    }
    let mut identity_order = records
        .iter()
        .map(|record| record.slot)
        .filter(|slot| !shard_free_slots.contains(slot.0))
        .collect::<Vec<_>>();
    identity_order.sort_by(|a, b| {
        records[a.0 as usize]
            .id
            .cmp(&records[b.0 as usize].id)
            .then_with(|| a.cmp(b))
    });
    let source_id = header.source_id.clone();
    let source_version = header.source_version;
    let role = header.role;
    let record_parses = header.record_parses;
    let failed_parses = header.failed_parses;
    let shared = header.shared;
    let mut shard = IndexedCollectionSegment::from_base_parts(BaseSegmentParts {
        collection: crate::model::CollectionId(collection),
        source_id,
        source_version,
        files,
        file_index,
        records,
        id_lookup,
        identity_order,
        postings,
        inference,
        custom_postings: custom,
        raw: raw_bigram,
        fallback_text: fallback_bigram,
        custom_text,
        logical_records: bitmaps
            .get(&(keys::BITMAP_LOGICAL_RECORDS, Vec::new()))
            .cloned()
            .unwrap_or_default(),
        empty_array_records: bitmaps
            .get(&(keys::BITMAP_EMPTY_ARRAY_RECORDS, Vec::new()))
            .cloned()
            .unwrap_or_default(),
        sorting,
        relations,
    });
    shard.role = match role {
        1 => ShardRole::Mutation,
        2 => ShardRole::Main,
        _ => ShardRole::Built,
    };
    shard.record_parses = record_parses;
    shard.failed_parses = failed_parses;
    shard.dense_epoch = header.dense_epoch;
    shard.free_slots = bitmaps
        .get(&(keys::BITMAP_FREE_SLOTS, Vec::new()))
        .cloned()
        .unwrap_or_default();
    shard.free_file_rows = bitmaps
        .get(&(keys::BITMAP_FREE_FILE_ROWS, Vec::new()))
        .cloned()
        .unwrap_or_default();
    if shared {
        shard.freeze_all();
    }
    shard.recompute_accounting();
    Ok((shard, active, header))
}

pub(crate) fn delete_shard_prefix(
    write: &WriteTransaction,
    collection: u32,
    store_id: u64,
) -> Result<(), String> {
    let prefix = keys::prefix(collection, store_id);
    let end = keys::prefix_end(&prefix).ok_or("shard prefix overflow")?;
    for definition in [
        keys::SHARDS,
        keys::ACTIVE,
        keys::FILES,
        keys::RECORDS,
        keys::POSTING_FIELDS,
        keys::POSTINGS,
        keys::BITMAPS,
        keys::SORT_COLUMNS,
        keys::SORT_KEYS,
        keys::COMPOSITES,
        keys::RELATIONS,
        keys::REFERENCES,
        keys::INFERENCE,
    ] {
        let mut table = write.open_table(definition).map_err(error)?;
        table
            .retain_in::<&[u8], _>(prefix.as_slice()..end.as_slice(), |_, _| false)
            .map_err(error)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "index_store_shard.test.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DiffReport {
    pub chunks_written: u64,
    pub chunks_deleted: u64,
    pub rows_written: u64,
    #[cfg(test)]
    pub records_written: u64,
}

fn delete_range(
    write: &WriteTransaction,
    definition: TableDefinition<&[u8], &[u8]>,
    start: &[u8],
    end: &[u8],
) -> Result<(), String> {
    let mut table = write.open_table(definition).map_err(error)?;
    table
        .retain_in::<&[u8], _>(start..end, |_, _| false)
        .map_err(error)?;
    Ok(())
}

fn field_prefix(prefix: &[u8], family: u8, field: &[u8]) -> Vec<u8> {
    let mut start = prefix.to_vec();
    start.push(family);
    keys::push_bytes(&mut start, field);
    start
}

struct MapTarget<'a> {
    prefix: &'a [u8],
    family: u8,
    field: &'a [u8],
}

fn diff_posting_map<K: Ord + Clone>(
    write: &WriteTransaction,
    target: MapTarget<'_>,
    old: &crate::posting_map::PostingMap<K>,
    new: &crate::posting_map::PostingMap<K>,
    encode_key: impl Fn(&K) -> Vec<u8>,
    report: &mut DiffReport,
) -> Result<(), String> {
    let MapTarget {
        prefix,
        family,
        field,
    } = target;
    let start = field_prefix(prefix, family, field);
    let end = keys::prefix_end(&start).ok_or("posting field prefix overflow")?;
    let compared = new
        .diff_chunks(old, |change| {
            match change {
                crate::posting_map::ChunkChange::Put(key, chunk, posting) => {
                    let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
                    table
                        .insert(
                            keys::posting_key(prefix, family, field, &encode_key(key), chunk)
                                .as_slice(),
                            bitmap_bytes(posting)?.as_slice(),
                        )
                        .map_err(error)?;
                    report.chunks_written += 1;
                }
                crate::posting_map::ChunkChange::Delete(key, chunk) => {
                    let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
                    table
                        .remove(
                            keys::posting_key(prefix, family, field, &encode_key(key), chunk)
                                .as_slice(),
                        )
                        .map_err(error)?;
                    report.chunks_deleted += 1;
                }
            }
            Ok(())
        })?;
    if !compared {
        delete_range(write, keys::POSTINGS, &start, &end)?;
        new.for_each_chunk(|key, chunk, posting| {
            let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
            table
                .insert(
                    keys::posting_key(prefix, family, field, &encode_key(key), chunk).as_slice(),
                    bitmap_bytes(posting)?.as_slice(),
                )
                .map_err(error)?;
            report.chunks_written += 1;
            Ok(())
        })?;
    }
    Ok(())
}

fn diff_gram_map(
    write: &WriteTransaction,
    prefix: &[u8],
    family: u8,
    field: &[u8],
    old: &crate::gram_map::GramMap,
    new: &crate::gram_map::GramMap,
    report: &mut DiffReport,
) -> Result<(), String> {
    let start = field_prefix(prefix, family, field);
    let end = keys::prefix_end(&start).ok_or("gram field prefix overflow")?;
    let compared = match (old.shared_map(), new.shared_map()) {
        (Some(old), Some(new)) => new.diff_chunks(old, |change| {
            match change {
                crate::posting_map::ChunkChange::Put(key, chunk, posting) => {
                    let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
                    table
                        .insert(
                            keys::posting_key(prefix, family, field, &key.to_be_bytes(), chunk)
                                .as_slice(),
                            bitmap_bytes(posting)?.as_slice(),
                        )
                        .map_err(error)?;
                    report.chunks_written += 1;
                }
                crate::posting_map::ChunkChange::Delete(key, chunk) => {
                    let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
                    table
                        .remove(
                            keys::posting_key(prefix, family, field, &key.to_be_bytes(), chunk)
                                .as_slice(),
                        )
                        .map_err(error)?;
                    report.chunks_deleted += 1;
                }
            }
            Ok(())
        })?,
        _ => false,
    };
    if !compared {
        delete_range(write, keys::POSTINGS, &start, &end)?;
        new.for_each_chunk(|key, chunk, posting| {
            let mut table = write.open_table(keys::POSTINGS).map_err(error)?;
            table
                .insert(
                    keys::posting_key(prefix, family, field, &key.to_be_bytes(), chunk).as_slice(),
                    bitmap_bytes(posting)?.as_slice(),
                )
                .map_err(error)?;
            report.chunks_written += 1;
            Ok(())
        })?;
    }
    Ok(())
}

fn write_posting_header(
    write: &WriteTransaction,
    prefix: &[u8],
    family: u8,
    field: &[u8],
    index: &PostingIndex,
    complete: Option<bool>,
    _report: &mut DiffReport,
) -> Result<(), String> {
    let mut table = write.open_table(keys::POSTING_FIELDS).map_err(error)?;
    table
        .insert(
            keys::posting_field_key(prefix, family, field).as_slice(),
            encode(&keys::PostingHeader {
                kind: index.kind.clone(),
                multi: index.multi,
                values_indexed: index.values_indexed,
                value_complete: index.value_complete,
                length_complete: index.length_complete,
                empty_string_complete: index.empty_string_complete,
                covered_records: index.covered_records,
                has_structural: index.structural.is_some(),
                custom_complete: complete,
            })?
            .as_slice(),
        )
        .map_err(error)?;
    Ok(())
}

fn write_bitmap_if_changed(
    write: &WriteTransaction,
    prefix: &[u8],
    kind: u8,
    field: &[u8],
    old: Option<&RoaringBitmap>,
    new: &RoaringBitmap,
    _report: &mut DiffReport,
) -> Result<(), String> {
    if old == Some(new) {
        return Ok(());
    }
    let mut table = write.open_table(keys::BITMAPS).map_err(error)?;
    table
        .insert(
            keys::bitmap_key(prefix, kind, field).as_slice(),
            bitmap_bytes(new)?.as_slice(),
        )
        .map_err(error)?;
    Ok(())
}

pub(crate) fn diff_main(
    write: &WriteTransaction,
    collection: u32,
    store_id: u64,
    old: &IndexedCollectionSegment,
    new: &IndexedCollectionSegment,
    old_active: &ActiveRecords,
    new_active: &ActiveRecords,
) -> Result<DiffReport, String> {
    let prefix = keys::prefix(collection, store_id);
    let mut report = DiffReport::default();
    if old_active.bitmap() != new_active.bitmap() {
        let mut table = write.open_table(keys::ACTIVE).map_err(error)?;
        table
            .insert(
                prefix.as_slice(),
                bitmap_bytes(&new_active.bitmap())?.as_slice(),
            )
            .map_err(error)?;
    }

    // Shard header and fixed shard bitmaps.
    let mut table = write.open_table(keys::SHARDS).map_err(error)?;
    table
        .insert(
            prefix.as_slice(),
            encode(&keys::ShardHeader {
                ordinal: 0,
                alphabet: new.raw.alphabet,
                role: match new.role {
                    ShardRole::Built => 0,
                    ShardRole::Mutation => 1,
                    ShardRole::Main => 2,
                },
                shared: new.identity_order().is_shared(),
                source_id: new.source_id.clone(),
                source_version: new.source_version,
                record_count: new.record_count() as u32,
                file_count: new.file_count() as u32,
                dense_epoch: new.dense_epoch,
                record_parses: new.record_parses,
                failed_parses: new.failed_parses,
                has_inference: new.inference.is_some(),
            })?
            .as_slice(),
        )
        .map_err(error)?;
    drop(table);
    write_bitmap_if_changed(
        write,
        &prefix,
        keys::BITMAP_LOGICAL_RECORDS,
        b"",
        Some(old.logical_records()),
        new.logical_records(),
        &mut report,
    )?;
    write_bitmap_if_changed(
        write,
        &prefix,
        keys::BITMAP_EMPTY_ARRAY_RECORDS,
        b"",
        Some(old.empty_array_records()),
        new.empty_array_records(),
        &mut report,
    )?;
    write_bitmap_if_changed(
        write,
        &prefix,
        keys::BITMAP_FREE_SLOTS,
        b"",
        Some(&old.free_slots),
        &new.free_slots,
        &mut report,
    )?;
    write_bitmap_if_changed(
        write,
        &prefix,
        keys::BITMAP_FREE_FILE_ROWS,
        b"",
        Some(&old.free_file_rows),
        &new.free_file_rows,
        &mut report,
    )?;

    // Declared and custom and inferred posting families.
    for (path, index) in &new.postings.fields {
        let field = field_bytes(path)?;
        let old_index = old.postings.fields.get(path);
        write_posting_header(
            write,
            &prefix,
            keys::FAMILY_DECLARED_VALUES,
            &field,
            index,
            None,
            &mut report,
        )?;
        let fallback_values = crate::posting_map::PostingMap::new();
        let fallback_lengths = crate::posting_map::PostingMap::new();
        let old_values = old_index.map_or(&fallback_values, |index| index.value_map());
        let old_lengths = old_index.map_or(&fallback_lengths, |index| index.length_map());
        diff_posting_map(
            write,
            MapTarget { prefix: &prefix, family: keys::FAMILY_DECLARED_VALUES, field: &field },
            old_values,
            index.value_map(),
            keys::encode_scalar_key,
            &mut report,
        )?;
        diff_posting_map(
            write,
            MapTarget { prefix: &prefix, family: keys::FAMILY_DECLARED_LENGTHS, field: &field },
            old_lengths,
            index.length_map(),
            keys::encode_number_value,
            &mut report,
        )?;
        let old_structural = old_index
            .and_then(|index| index.structural.as_ref())
            .map(|facts| facts.postings_map());
        let new_structural = index
            .structural
            .as_ref()
            .map(|facts| facts.postings_map());
        if let (Some(old_map), Some(new_map)) = (old_structural, new_structural) {
            diff_posting_map(
                write,
                MapTarget { prefix: &prefix, family: keys::FAMILY_DECLARED_STRUCTURAL, field: &field },
                old_map,
                new_map,
                |key| vec![structural_code(*key)],
                &mut report,
            )?;
        }
        for (kind, value) in [
            (keys::BITMAP_PRESENCE, &index.presence),
            (keys::BITMAP_NULLS, &index.nulls),
            (keys::BITMAP_EMPTY_SELECTIONS, &index.empty_selections),
        ] {
            let old_value = old_index.map(|index| match kind {
                keys::BITMAP_PRESENCE => &index.presence,
                keys::BITMAP_NULLS => &index.nulls,
                _ => &index.empty_selections,
            });
            write_bitmap_if_changed(write, &prefix, kind, &field, old_value, value, &mut report)?;
        }
    }
    for ((path, binding), custom) in &new.custom_postings {
        let field = custom_field_bytes(path, binding)?;
        let old_custom = old.custom_postings.get(&(path.clone(), binding.clone()));
        write_posting_header(
            write,
            &prefix,
            keys::FAMILY_CUSTOM_VALUES,
            &field,
            &custom.index,
            Some(custom.complete),
            &mut report,
        )?;
        let fallback_values = crate::posting_map::PostingMap::new();
        let fallback_lengths = crate::posting_map::PostingMap::new();
        let old_values = old_custom.map_or(&fallback_values, |custom| custom.index.value_map());
        let old_lengths = old_custom.map_or(&fallback_lengths, |custom| custom.index.length_map());
        diff_posting_map(
            write,
            MapTarget { prefix: &prefix, family: keys::FAMILY_CUSTOM_VALUES, field: &field },
            old_values,
            custom.index.value_map(),
            keys::encode_scalar_key,
            &mut report,
        )?;
        diff_posting_map(
            write,
            MapTarget { prefix: &prefix, family: keys::FAMILY_CUSTOM_LENGTHS, field: &field },
            old_lengths,
            custom.index.length_map(),
            keys::encode_number_value,
            &mut report,
        )?;
        for (kind, value) in [
            (keys::BITMAP_PRESENCE + 10, &custom.index.presence),
            (keys::BITMAP_NULLS + 10, &custom.index.nulls),
            (
                keys::BITMAP_EMPTY_SELECTIONS + 10,
                &custom.index.empty_selections,
            ),
        ] {
            let old_value = old_custom.map(|custom| {
                if kind == keys::BITMAP_PRESENCE + 10 {
                    &custom.index.presence
                } else if kind == keys::BITMAP_NULLS + 10 {
                    &custom.index.nulls
                } else {
                    &custom.index.empty_selections
                }
            });
            write_bitmap_if_changed(write, &prefix, kind, &field, old_value, value, &mut report)?;
        }
    }
    if let Some(inference) = &new.inference {
        let old_inference = old.inference.as_ref();
        for (path, index) in &inference.indexes {
            let field = field_bytes(path)?;
            let old_index = old_inference.and_then(|state| state.indexes.get(path));
            write_posting_header(
                write,
                &prefix,
                keys::FAMILY_INFERRED_VALUES,
                &field,
                index,
                None,
                &mut report,
            )?;
            let fallback_values = crate::posting_map::PostingMap::new();
            let fallback_lengths = crate::posting_map::PostingMap::new();
            let old_values = old_index.map_or(&fallback_values, |index| index.value_map());
            let old_lengths = old_index.map_or(&fallback_lengths, |index| index.length_map());
            diff_posting_map(
                write,
                MapTarget { prefix: &prefix, family: keys::FAMILY_INFERRED_VALUES, field: &field },
                old_values,
                index.value_map(),
                keys::encode_scalar_key,
                &mut report,
            )?;
            diff_posting_map(
                write,
                MapTarget { prefix: &prefix, family: keys::FAMILY_INFERRED_LENGTHS, field: &field },
                old_lengths,
                index.length_map(),
                keys::encode_number_value,
                &mut report,
            )?;
        }
        for (kind, map) in [
            (
                keys::BITMAP_INFERRED_EMPTY_SELECTIONS,
                &inference.empty_selections,
            ),
            (
                keys::BITMAP_INFERRED_SHORT_STRING_COVERAGE,
                &inference.short_string_coverage,
            ),
        ] {
            for (path, bitmap) in map {
                let field = field_bytes(path)?;
                let old_value = old_inference
                    .and_then(|state| {
                        if kind == keys::BITMAP_INFERRED_EMPTY_SELECTIONS {
                            state.empty_selections.get(path)
                        } else {
                            state.short_string_coverage.get(path)
                        }
                    });
                write_bitmap_if_changed(
                    write,
                    &prefix,
                    kind,
                    &field,
                    old_value,
                    bitmap,
                    &mut report,
                )?;
            }
        }
        let old_report = old_inference.map(|state| encode(&state.report)).transpose()?;
        let new_report = encode(&inference.report)?;
        if old_report.as_deref() != Some(new_report.as_slice()) {
            let mut table = write.open_table(keys::INFERENCE).map_err(error)?;
            table
                .insert(prefix.as_slice(), new_report.as_slice())
                .map_err(error)?;
            report.rows_written += 1;
        }
    }

    // Gram families.
    for (base, old_index, new_index) in [
        (keys::FAMILY_RAW_NORMAL, &old.raw, &new.raw),
        (
            keys::FAMILY_FALLBACK_NORMAL,
            &old.fallback_text,
            &new.fallback_text,
        ),
    ] {
        for skip in [false, true] {
            let family = if skip { base + 1 } else { base };
            diff_gram_map(
                write,
                &prefix,
                family,
                b"",
                old_index.gram_map(skip),
                new_index.gram_map(skip),
                &mut report,
            )?;
        }
    }
    for ((path, binding), index) in &new.custom_text {
        let field = custom_field_bytes(path, binding)?;
        let old_index = old.custom_text.get(&(path.clone(), binding.clone()));
        for skip in [false, true] {
            let family = keys::FAMILY_CUSTOM_TEXT_NORMAL + u8::from(skip);
            let fallback = crate::gram_map::GramMap::default();
            let old_map = old_index.map_or(&fallback, |index| index.gram_map(skip));
            diff_gram_map(
                write,
                &prefix,
                family,
                &field,
                old_map,
                index.gram_map(skip),
                &mut report,
            )?;
        }
    }

    // Files and records.
    let mut files = write.open_table(keys::FILES).map_err(error)?;
    for (position, file) in new.files_iter().enumerate() {
        files
            .insert(
                keys::files_key(&prefix, position as u32).as_slice(),
                encode(file)?.as_slice(),
            )
            .map_err(error)?;
        report.rows_written += 1;
    }
    for position in new.file_count()..old.file_count() {
        files
            .remove(keys::files_key(&prefix, position as u32).as_slice())
            .map_err(error)?;
    }
    drop(files);
    let mut records = write.open_table(keys::RECORDS).map_err(error)?;
    if new.dense_epoch != old.dense_epoch {
        let end = keys::prefix_end(&prefix).unwrap();
        records
            .retain_in::<&[u8], _>(prefix.as_slice()..end.as_slice(), |_, _| false)
            .map_err(error)?;
        for slot in 0..new.record_count() {
            let record = new.record(RecordSlot(slot as u32)).ok_or("new record missing")?;
            records
                .insert(
                    keys::records_key(&prefix, slot as u32).as_slice(),
                    encode(record)?.as_slice(),
                )
                .map_err(error)?;
            report.rows_written += 1;
            #[cfg(test)]
            {
                report.records_written += 1;
            }
        }
    } else {
        for slot in old.record_count()..new.record_count() {
            let record = new.record(RecordSlot(slot as u32)).ok_or("new record missing")?;
            records
                .insert(
                    keys::records_key(&prefix, slot as u32).as_slice(),
                    encode(record)?.as_slice(),
                )
                .map_err(error)?;
            report.rows_written += 1;
            #[cfg(test)]
            {
                report.records_written += 1;
            }
        }
        for slot in new.reuse_log.iter() {
            if slot.0 < old.record_count() as u32 {
                let record = new.record(*slot).ok_or("reused record missing")?;
                records
                    .insert(
                        keys::records_key(&prefix, slot.0).as_slice(),
                        encode(record)?.as_slice(),
                    )
                    .map_err(error)?;
                report.rows_written += 1;
                #[cfg(test)]
                {
                    report.records_written += 1;
                }
            }
        }
    }
    drop(records);

    // Sort keys for new slots only.
    let mut sort_keys = write.open_table(keys::SORT_KEYS).map_err(error)?;
    for column in new.sorting.columns.values() {
        let column_bytes = encode(&column.path)?;
        let slots: Vec<u32> = if new.dense_epoch != old.dense_epoch {
            let mut start = prefix.to_vec();
            keys::push_bytes(&mut start, &column_bytes);
            let end = keys::prefix_end(&start).unwrap();
            sort_keys
                .retain_in::<&[u8], _>(start.as_slice()..end.as_slice(), |_, _| false)
                .map_err(error)?;
            (0..new.record_count() as u32).collect()
        } else {
            let mut slots = (old.record_count() as u32..new.record_count() as u32).collect::<Vec<_>>();
            for slot in new.reuse_log.iter() {
                if slot.0 < old.record_count() as u32 && !slots.contains(&slot.0) {
                    slots.push(slot.0);
                }
            }
            slots
        };
        for slot in slots {
            let key = column.key(RecordSlot(slot)).ok_or("new sort key missing")?;
            sort_keys
                .insert(
                    keys::sort_key_key(&prefix, &column_bytes, slot).as_slice(),
                    keys::encode_sort_key(key).as_slice(),
                )
                .map_err(error)?;
            report.rows_written += 1;
        }
    }
    drop(sort_keys);
    let mut columns = write.open_table(keys::SORT_COLUMNS).map_err(error)?;
    for column in new.sorting.columns.values() {
        let column_bytes = encode(&column.path)?;
        columns
            .insert(
                keys::sort_column_key(&prefix, &column_bytes).as_slice(),
                encode(&keys::SortColumnHeader {
                    path: column.path.clone(),
                    kind: column.kind.clone(),
                    locale: column.locale.clone(),
                    options: column.options,
                    collation_version: column.collation_version.clone(),
                })?
                .as_slice(),
            )
            .map_err(error)?;
    }
    drop(columns);
    let mut composites = write.open_table(keys::COMPOSITES).map_err(error)?;
    for (index, composite) in new.sorting.composites.iter().enumerate() {
        composites
            .insert(
                keys::composite_key(&prefix, index as u32).as_slice(),
                encode(&composite.fields)?.as_slice(),
            )
            .map_err(error)?;
    }
    drop(composites);

    // Relations.
    let mut relations = write.open_table(keys::RELATIONS).map_err(error)?;
    let mut references = write.open_table(keys::REFERENCES).map_err(error)?;
    for (ordinal, relation) in new.relations().iter().enumerate() {
        relations
            .insert(
                keys::relation_key(&prefix, ordinal as u32).as_slice(),
                encode(&keys::RelationHeader {
                    declaration: relation.declaration.clone(),
                    record_count: relation.record_count,
                    reference_count: relation.reference_rows().count() as u32,
                })?
                .as_slice(),
            )
            .map_err(error)?;
        for (kind, bitmap) in [
            (keys::BITMAP_RELATION_PRESENT, &relation.present),
            (keys::BITMAP_RELATION_NULLS, &relation.nulls),
            (keys::BITMAP_RELATION_EMPTY_ARRAYS, &relation.empty_arrays),
            (keys::BITMAP_RELATION_UNAVAILABLE, &relation.unavailable),
        ] {
            let old_value = old.relations().get(ordinal).map(|old| match kind {
                keys::BITMAP_RELATION_PRESENT => &old.present,
                keys::BITMAP_RELATION_NULLS => &old.nulls,
                keys::BITMAP_RELATION_EMPTY_ARRAYS => &old.empty_arrays,
                _ => &old.unavailable,
            });
            write_bitmap_if_changed(
                write,
                &prefix,
                kind,
                &(ordinal as u32).to_be_bytes(),
                old_value,
                bitmap,
                &mut report,
            )?;
        }
        if new.dense_epoch != old.dense_epoch {
            let mut start = prefix.to_vec();
            keys::push_u32(&mut start, ordinal as u32);
            let end = keys::prefix_end(&start).unwrap();
            references
                .retain_in::<&[u8], _>(start.as_slice()..end.as_slice(), |_, _| false)
                .map_err(error)?;
        } else {
            let mut delete_slots = (old.record_count() as u32..new.record_count() as u32)
                .collect::<Vec<_>>();
            for slot in new.reuse_log.iter() {
                if slot.0 < old.record_count() as u32 {
                    delete_slots.push(slot.0);
                }
            }
            for slot in delete_slots {
                let source = RecordSlot(slot);
                let mut start = prefix.to_vec();
                keys::push_u32(&mut start, ordinal as u32);
                keys::push_u32(&mut start, source.0);
                let end = keys::prefix_end(&start).unwrap();
                references
                    .retain_in::<&[u8], _>(start.as_slice()..end.as_slice(), |_, _| false)
                    .map_err(error)?;
            }
        }
        let rows = relation.reference_rows().collect::<Vec<_>>();
        for (index, (source, target, positions)) in rows.iter().enumerate() {
            if new.dense_epoch == old.dense_epoch
                && source.0 < old.record_count() as u32
                && !new.reuse_log.iter().any(|slot| slot.0 == source.0)
            {
                continue;
            }
            references
                .insert(
                    keys::reference_key(&prefix, ordinal as u32, source.0, index as u32).as_slice(),
                    encode(&PendingReference {
                        source: *source,
                        target_id: (*target).to_owned(),
                        positions: positions.to_vec(),
                    })?
                    .as_slice(),
                )
                .map_err(error)?;
            report.rows_written += 1;
        }
    }
    drop(relations);
    drop(references);
    Ok(report)
}
