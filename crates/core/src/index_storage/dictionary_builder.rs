//! Bulk construction of immutable ordered dictionaries from sorted entries.

use std::cmp::Ordering;

use super::dictionary::{
    BLOB_INLINE, BLOB_OVERFLOW, DEFAULT_PAGE_SIZE, DICTIONARY_HEADER_LEN, DICTIONARY_MAGIC,
    DictionaryHeader, INLINE_BLOB_LIMIT, MAX_PAGE_SIZE, PAGE_HEADER_LEN, PAGE_INTERNAL, PAGE_LEAF,
};
use super::error::{StorageError, StorageResult};
use super::scalar::KeyOrder;


#[derive(Clone)]
struct Child {
    offset: u64,
    first_key: Vec<u8>,
    last_key: Vec<u8>,
}


struct PageDraft {
    bytes: Vec<u8>,
    offset: u64,
    first_key: Vec<u8>,
    last_key: Vec<u8>,
}


/// Bulk-build one immutable dictionary from strictly increasing entries.
///
/// Entries are encoded as supplied; callers own key encoding and must sort by
/// the same [`KeyOrder`]. Builder collections are released when this function
/// returns, leaving only the packed page bytes.
pub fn build_dictionary(
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    order: KeyOrder,
    page_target: u32,
) -> StorageResult<Vec<u8>> {
    if !(1024..=MAX_PAGE_SIZE).contains(&page_target) {
        return Err(StorageError::InvalidArgument(format!(
            "dictionary page target {page_target} is out of range"
        )));
    }
    let page_target = page_target.max(DEFAULT_PAGE_SIZE.div_euclid(2)) as usize;
    let mut previous: Option<&[u8]> = None;
    for (key, _) in &entries {
        if let Some(previous) = previous {
            if order.compare(previous, key)? != Ordering::Less {
                return Err(StorageError::InvalidArgument(
                    "dictionary entries must be strictly increasing".into(),
                ));
            }
        }
        previous = Some(key);
    }
    let entry_count = entries.len() as u64;
    let key_bytes = entries
        .iter()
        .try_fold(0u64, |total, (key, _)| total.checked_add(key.len() as u64))
        .ok_or_else(|| StorageError::ResourceLimit("dictionary key bytes overflow".into()))?;
    let mut overflow = Vec::new();
    let mut cursor = DICTIONARY_HEADER_LEN as u64;
    let leaves = build_leaf_level(&entries, page_target, &mut overflow, &mut cursor)?;
    let mut pages = leaves;
    let mut depth = 0u32;
    let mut children = pages
        .iter()
        .map(|page| Child {
            offset: page.offset,
            first_key: page.first_key.clone(),
            last_key: page.last_key.clone(),
        })
        .collect::<Vec<_>>();
    while children.len() > 1 {
        let parents = build_internal_level(&children, page_target, &mut overflow, &mut cursor)?;
        depth = depth
            .checked_add(1)
            .ok_or_else(|| StorageError::ResourceLimit("dictionary depth overflow".into()))?;
        if depth > super::dictionary::MAX_DEPTH {
            return Err(StorageError::ResourceLimit(
                "dictionary depth exceeds the supported maximum".into(),
            ));
        }
        children = parents
            .iter()
            .map(|page| Child {
                offset: page.offset,
                first_key: page.first_key.clone(),
                last_key: page.last_key.clone(),
            })
            .collect();
        pages.extend(parents);
    }
    let root = pages
        .last()
        .ok_or_else(|| StorageError::corrupt("dictionary builder produced no root page"))?;
    let overflow_offset = align(cursor, 8)?;
    let overflow_len = overflow.len() as u64;
    let total_len = overflow_offset
        .checked_add(overflow_len)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary length overflow".into()))?;
    let total_len = usize::try_from(total_len)
        .map_err(|_| StorageError::ResourceLimit("dictionary does not fit in memory".into()))?;
    let mut bytes = vec![0u8; total_len];
    let header = DictionaryHeader::new(
        order,
        page_target as u32,
        root.offset,
        root.bytes.len() as u32,
        depth,
        entry_count,
        key_bytes,
        pages.len() as u64,
        overflow_offset,
        overflow_len,
    );
    header.write(&mut bytes[..DICTIONARY_HEADER_LEN]);
    for page in &pages {
        let start = page.offset as usize;
        let end = start
            .checked_add(page.bytes.len())
            .ok_or_else(|| StorageError::ResourceLimit("dictionary page end overflow".into()))?;
        bytes[start..end].copy_from_slice(&page.bytes);
    }
    let start = overflow_offset as usize;
    bytes[start..start + overflow.len()].copy_from_slice(&overflow);
    if bytes[..4] != DICTIONARY_MAGIC {
        return Err(StorageError::corrupt("dictionary builder lost its magic"));
    }
    Ok(bytes)
}

fn build_leaf_level(
    entries: &[(Vec<u8>, Vec<u8>)],
    page_target: usize,
    overflow: &mut Vec<u8>,
    cursor: &mut u64,
) -> StorageResult<Vec<PageDraft>> {
    let mut encoded = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        encoded.push(EncodedEntry {
            bytes: encode_leaf_entry(key, value, overflow)?,
            key: key.clone(),
        });
    }
    let mut groups: Vec<Vec<EncodedEntry>> = Vec::new();
    let mut current = Vec::new();
    let mut current_len = PAGE_HEADER_LEN;
    for entry in encoded {
        let extra = 4usize
            .checked_add(entry.bytes.len())
            .ok_or_else(|| StorageError::ResourceLimit("dictionary page entry overflow".into()))?;
        if !current.is_empty()
            && (current_len.saturating_add(extra) > page_target
                || current.len() >= u16::MAX as usize)
        {
            groups.push(std::mem::take(&mut current));
            current_len = PAGE_HEADER_LEN;
        }
        current_len = current_len.saturating_add(extra);
        current.push(entry);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    if groups.is_empty() {
        groups.push(Vec::new());
    }
    let mut pages = Vec::with_capacity(groups.len());
    for (number, group) in groups.into_iter().enumerate() {
        let bytes = encode_leaf_page(&group)?;
        let offset = *cursor;
        *cursor = align(
            cursor.checked_add(bytes.len() as u64).ok_or_else(|| {
                StorageError::ResourceLimit("dictionary page offset overflow".into())
            })?,
            8,
        )?;
        let first_key = group
            .first()
            .map(|entry| entry.key.clone())
            .unwrap_or_default();
        let last_key = group
            .last()
            .map(|entry| entry.key.clone())
            .unwrap_or_default();
        let _ = number;
        pages.push(PageDraft {
            bytes,
            offset,
            first_key,
            last_key,
        });
    }
    for index in 0..pages.len() {
        let next = pages.get(index + 1).map_or(0, |page| page.offset);
        let previous = index.checked_sub(1).map_or(0, |index| pages[index].offset);
        pages[index].bytes[8..16].copy_from_slice(&next.to_le_bytes());
        pages[index].bytes[16..24].copy_from_slice(&previous.to_le_bytes());
    }
    Ok(pages)
}


struct EncodedEntry {
    bytes: Vec<u8>,
    key: Vec<u8>,
}


fn build_internal_level(
    children: &[Child],
    page_target: usize,
    overflow: &mut Vec<u8>,
    cursor: &mut u64,
) -> StorageResult<Vec<PageDraft>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current = Vec::new();
    let mut current_len = PAGE_HEADER_LEN;
    for (index, child) in children.iter().enumerate() {
        // The rightmost child of one page has no separator key; estimate with
        // a separator for every other child.
        let entry_len = if current.is_empty() {
            0
        } else {
            internal_entry_len(&child.last_key)
        };
        let extra = entry_len.saturating_add(4);
        if current.len() >= 2 && current_len.saturating_add(extra) > page_target {
            groups.push(std::mem::take(&mut current));
            current_len = PAGE_HEADER_LEN;
        }
        current_len = current_len.saturating_add(extra);
        current.push(index);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    if groups.len() > 1 && groups.last().is_some_and(|group| group.len() == 1) {
        let tail = groups.pop().expect("last group");
        groups.last_mut().expect("previous group").extend(tail);
    }
    let mut pages = Vec::with_capacity(groups.len());
    for group in groups {
        let group_children = group
            .iter()
            .map(|index| children[*index].clone())
            .collect::<Vec<_>>();
        let bytes = encode_internal_page(&group_children, overflow)?;
        let offset = *cursor;
        *cursor = align(
            cursor.checked_add(bytes.len() as u64).ok_or_else(|| {
                StorageError::ResourceLimit("dictionary page offset overflow".into())
            })?,
            8,
        )?;
        let first_key = group_children
            .first()
            .map(|child| child.first_key.clone())
            .unwrap_or_default();
        let last_key = group_children
            .last()
            .map(|child| child.last_key.clone())
            .unwrap_or_default();
        pages.push(PageDraft {
            bytes,
            offset,
            first_key,
            last_key,
        });
    }
    Ok(pages)
}

fn internal_entry_len(key: &[u8]) -> usize {
    8 + 4
        + 1
        + 3
        + if key.len() <= INLINE_BLOB_LIMIT {
            key.len()
        } else {
            8
        }
}

fn encode_leaf_entry(key: &[u8], value: &[u8], overflow: &mut Vec<u8>) -> StorageResult<Vec<u8>> {
    let (key_kind, key_reference) = blob_reference(key, overflow)?;
    let (value_kind, value_reference) = blob_reference(value, overflow)?;
    let mut bytes =
        Vec::with_capacity(4 + 4 + 4 + key_reference.len() + 1 + 3 + value_reference.len());
    let key_len = u32::try_from(key.len())
        .map_err(|_| StorageError::ResourceLimit("dictionary key exceeds 4 GiB".into()))?;
    let value_len = u32::try_from(value.len())
        .map_err(|_| StorageError::ResourceLimit("dictionary value exceeds 4 GiB".into()))?;
    bytes.extend_from_slice(&key_len.to_le_bytes());
    bytes.push(key_kind);
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&value_len.to_le_bytes());
    bytes.extend_from_slice(&key_reference);
    bytes.push(value_kind);
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&value_reference);
    Ok(bytes)
}

fn encode_internal_entry(child: &Child, overflow: &mut Vec<u8>) -> StorageResult<Vec<u8>> {
    let (key_kind, key_reference) = blob_reference(&child.last_key, overflow)?;
    let key_len = u32::try_from(child.last_key.len())
        .map_err(|_| StorageError::ResourceLimit("dictionary key exceeds 4 GiB".into()))?;
    let mut bytes = Vec::with_capacity(8 + 4 + 4 + key_reference.len());
    bytes.extend_from_slice(&child.offset.to_le_bytes());
    bytes.extend_from_slice(&key_len.to_le_bytes());
    bytes.push(key_kind);
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes.extend_from_slice(&key_reference);
    Ok(bytes)
}

fn blob_reference(kind_value: &[u8], overflow: &mut Vec<u8>) -> StorageResult<(u8, Vec<u8>)> {
    if kind_value.len() > INLINE_BLOB_LIMIT {
        let offset = u64::try_from(overflow.len())
            .map_err(|_| StorageError::ResourceLimit("overflow arena length overflow".into()))?;
        overflow.extend_from_slice(kind_value);
        Ok((BLOB_OVERFLOW, offset.to_le_bytes().to_vec()))
    } else {
        Ok((BLOB_INLINE, kind_value.to_vec()))
    }
}

fn encode_leaf_page(entries: &[EncodedEntry]) -> StorageResult<Vec<u8>> {
    let count = u16::try_from(entries.len()).map_err(|_| {
        StorageError::ResourceLimit("dictionary leaf page has too many entries".into())
    })?;
    let header_len = PAGE_HEADER_LEN
        .checked_add(entries.len() * 4)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary leaf page overflow".into()))?;
    let entry_bytes = entries
        .iter()
        .try_fold(0usize, |total, entry| total.checked_add(entry.bytes.len()))
        .ok_or_else(|| StorageError::ResourceLimit("dictionary leaf page overflow".into()))?;
    let total = header_len
        .checked_add(entry_bytes)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary leaf page overflow".into()))?;
    let mut bytes = vec![0u8; total];
    bytes[0] = PAGE_LEAF;
    bytes[1..3].copy_from_slice(&count.to_le_bytes());
    bytes[3..8].fill(0);
    let data_len = u32::try_from(total)
        .map_err(|_| StorageError::ResourceLimit("dictionary page exceeds 4 GiB".into()))?;
    bytes[24..28].copy_from_slice(&data_len.to_le_bytes());
    bytes[28..32].fill(0);
    let mut at = header_len;
    for (index, entry) in entries.iter().enumerate() {
        let offset = u32::try_from(at).map_err(|_| {
            StorageError::ResourceLimit("dictionary page entry offset overflow".into())
        })?;
        let slot = PAGE_HEADER_LEN + index * 4;
        bytes[slot..slot + 4].copy_from_slice(&offset.to_le_bytes());
        bytes[at..at + entry.bytes.len()].copy_from_slice(&entry.bytes);
        at += entry.bytes.len();
    }
    Ok(bytes)
}

fn encode_internal_page(children: &[Child], overflow: &mut Vec<u8>) -> StorageResult<Vec<u8>> {
    if children.is_empty() {
        return Err(StorageError::corrupt(
            "dictionary builder produced an empty internal page",
        ));
    }
    let key_count = children.len() - 1;
    let entries = children[..key_count]
        .iter()
        .map(|child| encode_internal_entry(child, overflow))
        .collect::<StorageResult<Vec<_>>>()?;
    let count = u16::try_from(key_count).map_err(|_| {
        StorageError::ResourceLimit("dictionary internal page has too many keys".into())
    })?;
    let header_len = PAGE_HEADER_LEN
        .checked_add(entries.len() * 4)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary internal page overflow".into()))?;
    let entry_bytes = entries
        .iter()
        .try_fold(0usize, |total, entry| total.checked_add(entry.len()))
        .ok_or_else(|| StorageError::ResourceLimit("dictionary internal page overflow".into()))?;
    let total = header_len
        .checked_add(entry_bytes)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary internal page overflow".into()))?;
    let mut bytes = vec![0u8; total];
    bytes[0] = PAGE_INTERNAL;
    bytes[1..3].copy_from_slice(&count.to_le_bytes());
    bytes[3..8].fill(0);
    let rightmost = children.last().expect("nonempty children").offset;
    bytes[8..16].copy_from_slice(&rightmost.to_le_bytes());
    bytes[16..24].fill(0);
    let data_len = u32::try_from(total)
        .map_err(|_| StorageError::ResourceLimit("dictionary page exceeds 4 GiB".into()))?;
    bytes[24..28].copy_from_slice(&data_len.to_le_bytes());
    bytes[28..32].fill(0);
    let mut at = header_len;
    for (index, entry) in entries.iter().enumerate() {
        let offset = u32::try_from(at).map_err(|_| {
            StorageError::ResourceLimit("dictionary page entry offset overflow".into())
        })?;
        let slot = PAGE_HEADER_LEN + index * 4;
        bytes[slot..slot + 4].copy_from_slice(&offset.to_le_bytes());
        bytes[at..at + entry.len()].copy_from_slice(entry);
        at += entry.len();
    }
    Ok(bytes)
}

fn align(value: u64, alignment: u64) -> StorageResult<u64> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| StorageError::ResourceLimit("dictionary alignment overflow".into()))
}
