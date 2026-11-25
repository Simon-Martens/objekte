//! One scalar membership per physical file, independent of record cardinality.

use roaring::RoaringBitmap;
use std::cmp::Ordering;

use crate::model::{FileId, Number};
use crate::query::Operator;

use crate::inventory::FileEntry;

#[derive(Clone, Debug, Default)]
pub(crate) struct FileScalarIndex {
    sizes: Vec<(u64, FileId)>,
    modified: Vec<(i128, FileId)>,
    created: Vec<(i128, FileId)>,
    unknown_created: Vec<FileId>,
}

fn insert_sorted<K: Ord>(rows: &mut Vec<(K, FileId)>, key: K, id: FileId) {
    let position = rows
        .binary_search_by(|(value, file)| value.cmp(&key).then(file.cmp(&id)))
        .unwrap_or_else(|position| position);
    rows.insert(position, (key, id));
}

fn remove_sorted<K: Ord>(rows: &mut Vec<(K, FileId)>, key: K, id: FileId) {
    if let Ok(position) = rows.binary_search(&(key, id)) {
        rows.remove(position);
    }
}

fn collect<K: Ord>(
    rows: &[(K, FileId)],
    operator: Operator,
    literal: &K,
    output: &mut Vec<FileId>,
    limit: usize,
    live: Option<&RoaringBitmap>,
) -> bool {
    collect_by(rows, operator, output, limit, live, |value| {
        value.cmp(literal)
    })
}

fn collect_by<K>(
    rows: &[(K, FileId)],
    operator: Operator,
    output: &mut Vec<FileId>,
    limit: usize,
    live: Option<&RoaringBitmap>,
    compare: impl Fn(&K) -> Ordering,
) -> bool {
    let lower = rows.partition_point(|(value, _)| compare(value).is_lt());
    let upper = rows.partition_point(|(value, _)| !compare(value).is_gt());
    let slices: [&[(K, FileId)]; 2] = match operator {
        Operator::Eq => [&rows[lower..upper], &[]],
        Operator::Ne => [&rows[..lower], &rows[upper..]],
        Operator::Gt => [&rows[upper..], &[]],
        Operator::Ge => [&rows[lower..], &[]],
        Operator::Lt => [&rows[..lower], &[]],
        Operator::Le => [&rows[..upper], &[]],
        _ => return false,
    };
    for slice in slices {
        if let Some(live) = live {
            if operator == Operator::Eq && slice.len() >= 64 {
                let first = slice.first().expect("nonempty tie").1.0;
                let last = slice.last().expect("nonempty tie").1.0;
                let range = first..=last;
                if live.range_cardinality(range.clone()) < (slice.len() / 4) as u64 {
                    for id in live.range(range).map(FileId) {
                        if slice.binary_search_by_key(&id, |(_, file)| *file).is_ok() {
                            if output.len() == limit {
                                return false;
                            }
                            output.push(id);
                        }
                    }
                    continue;
                }
            }
            for (_, id) in slice {
                if live.contains(id.0) {
                    if output.len() == limit {
                        return false;
                    }
                    output.push(*id);
                }
            }
        } else {
            if slice.len() > limit.saturating_sub(output.len()) {
                return false;
            }
            output.extend(slice.iter().map(|(_, id)| *id));
        }
    }
    true
}

impl FileScalarIndex {
    pub(crate) fn estimated_heap_bytes(&self) -> usize {
        self.sizes.capacity() * std::mem::size_of::<(u64, FileId)>()
            + self.modified.capacity() * std::mem::size_of::<(i128, FileId)>()
            + self.created.capacity() * std::mem::size_of::<(i128, FileId)>()
            + self.unknown_created.capacity() * std::mem::size_of::<FileId>()
    }

    pub(crate) fn from_files(files: &[FileEntry]) -> Self {
        let mut index = Self::default();
        index.sizes.reserve(files.len());
        index.modified.reserve(files.len());
        index.created.reserve(files.len());
        for file in files {
            index.sizes.push((file.revision.size, file.id));
            index.modified.push((file.revision.modified_ns, file.id));
            if let Some(created) = file.created_ns {
                index.created.push((created, file.id));
            } else {
                index.unknown_created.push(file.id);
            }
        }
        index.sizes.sort();
        index.modified.sort();
        index.created.sort();
        index.unknown_created.sort();
        index
    }

    pub(crate) fn insert(&mut self, file: &FileEntry) {
        insert_sorted(&mut self.sizes, file.revision.size, file.id);
        insert_sorted(&mut self.modified, file.revision.modified_ns, file.id);
        if let Some(created) = file.created_ns {
            insert_sorted(&mut self.created, created, file.id);
        } else {
            let position = self
                .unknown_created
                .binary_search(&file.id)
                .unwrap_or_else(|position| position);
            self.unknown_created.insert(position, file.id);
        }
    }

    pub(crate) fn remove(&mut self, file: &FileEntry) {
        remove_sorted(&mut self.sizes, file.revision.size, file.id);
        remove_sorted(&mut self.modified, file.revision.modified_ns, file.id);
        if let Some(created) = file.created_ns {
            remove_sorted(&mut self.created, created, file.id);
        } else if let Ok(position) = self.unknown_created.binary_search(&file.id) {
            self.unknown_created.remove(position);
        }
    }

    pub(crate) fn size_ids(
        &self,
        operator: Operator,
        literal: &Number,
        output: &mut Vec<FileId>,
        limit: usize,
        live: Option<&RoaringBitmap>,
    ) -> bool {
        collect_by(&self.sizes, operator, output, limit, live, |size| {
            Number::integer(*size as i128).cmp(literal)
        })
    }

    pub(crate) fn modified_ids(
        &self,
        operator: Operator,
        literal: i128,
        output: &mut Vec<FileId>,
        limit: usize,
        live: Option<&RoaringBitmap>,
    ) -> bool {
        collect(&self.modified, operator, &literal, output, limit, live)
    }

    pub(crate) fn created_ids(
        &self,
        operator: Operator,
        literal: i128,
        output: &mut Vec<FileId>,
        limit: usize,
        live: Option<&RoaringBitmap>,
    ) -> bool {
        collect(&self.created, operator, &literal, output, limit, live)
    }

    pub(crate) fn unknown_created_ids(
        &self,
        output: &mut Vec<FileId>,
        limit: usize,
        live: Option<&RoaringBitmap>,
    ) -> bool {
        if let Some(live) = live {
            if let (Some(first), Some(last)) =
                (self.unknown_created.first(), self.unknown_created.last())
                && live.range_cardinality(first.0..=last.0)
                    < (self.unknown_created.len() / 4) as u64
            {
                for id in live.range(first.0..=last.0).map(FileId) {
                    if self.unknown_created.binary_search(&id).is_ok() {
                        if output.len() == limit {
                            return false;
                        }
                        output.push(id);
                    }
                }
            } else {
                for id in &self.unknown_created {
                    if live.contains(id.0) {
                        if output.len() == limit {
                            return false;
                        }
                        output.push(*id);
                    }
                }
            }
        } else {
            if self.unknown_created.len() > limit.saturating_sub(output.len()) {
                return false;
            }
            output.extend(self.unknown_created.iter().copied());
        }
        true
    }
}
