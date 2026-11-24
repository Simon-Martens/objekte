use std::collections::HashMap;
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::{
    COLLATION_VERSION, Placement, SortColumn, SortConfig, SortKey, SortOptions, make_collator,
};
use crate::index::posting::{PostingIndex, ScalarKey};
use crate::inventory::RecordClass;
use crate::model::{FieldPath, LogicalType, Number, RecordSlot};
use crate::records::RecordIdSlot;

/// An inferred order is query-created but remains valid for a repository
/// generation. Cached columns use the same representation as configured
/// columns, so paging and tie-breaking follow the existing execution path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InferredOrderKey {
    collection: usize,
    generation: u64,
    segment: crate::index::segment::SegmentId,
    path: FieldPath,
    locale: String,
    collation_version: &'static str,
    options: SortOptions,
    descending: bool,
}

impl InferredOrderKey {
    pub(crate) fn new(
        collection: usize,
        generation: u64,
        segment: crate::index::segment::SegmentId,
        path: FieldPath,
        locale: &str,
        options: SortOptions,
        descending: bool,
    ) -> Self {
        Self {
            collection,
            generation,
            segment,
            path,
            locale: locale.to_owned(),
            collation_version: COLLATION_VERSION,
            options,
            descending,
        }
    }

    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.locale.capacity())
            .saturating_add(
                self.path
                    .segments
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(
                self.path
                    .segments
                    .iter()
                    .fold(0usize, |bytes, name| bytes.saturating_add(name.capacity())),
            )
    }
}

struct InferredOrderEntry {
    column: Arc<SortColumn>,
    bytes: usize,
    last_used: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InferredOrderCacheStats {
    pub builds: u64,
    pub hits: u64,
    pub evictions: u64,
    pub resident_bytes: usize,
    pub entries: usize,
}

/// A lazy, bounded cache owned by one repository. Callers build a missing
/// column outside its mutex, then publish it with a second short lock.
pub(crate) struct InferredOrderCache {
    entries: HashMap<InferredOrderKey, InferredOrderEntry>,
    max_bytes: usize,
    used_bytes: usize,
    clock: u64,
    stats: InferredOrderCacheStats,
}

impl InferredOrderCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_bytes,
            used_bytes: 0,
            clock: 0,
            stats: InferredOrderCacheStats::default(),
        }
    }

    pub(crate) fn get(&mut self, key: &InferredOrderKey) -> Option<Arc<SortColumn>> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.clock;
            self.stats.hits += 1;
            return Some(entry.column.clone());
        }
        None
    }

    /// A concurrent builder may already have published this key. Reuse that
    /// column and charge only one resident entry, while still counting both
    /// completed constructions in the structural build counter.
    pub(crate) fn insert_if_absent(
        &mut self,
        key: InferredOrderKey,
        column: Arc<SortColumn>,
    ) -> Arc<SortColumn> {
        self.stats.builds += 1;
        if let Some(existing) = self.get(&key) {
            return existing;
        }
        let bytes = Self::entry_bytes(&key, &column);
        if bytes <= self.max_bytes {
            while self.used_bytes.saturating_add(bytes) > self.max_bytes {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| key.clone())
                    .expect("cache has an entry to evict");
                let removed = self.entries.remove(&oldest).expect("oldest cache entry");
                self.used_bytes -= removed.bytes;
                self.stats.evictions += 1;
            }
            self.used_bytes += bytes;
            self.entries.insert(
                key,
                InferredOrderEntry {
                    column: column.clone(),
                    bytes,
                    last_used: self.clock,
                },
            );
        }
        self.stats.resident_bytes = self.used_bytes;
        self.stats.entries = self.entries.len();
        column
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> InferredOrderCacheStats {
        self.stats
    }

    fn entry_bytes(key: &InferredOrderKey, column: &SortColumn) -> usize {
        // Charge two map buckets per entry to cover spare hash-table capacity,
        // plus the Arc header. The string and vector capacities are counted by
        // the key and column themselves.
        let map_bytes = std::mem::size_of::<InferredOrderKey>()
            .saturating_add(std::mem::size_of::<InferredOrderEntry>())
            .saturating_mul(2);
        key.retained_bytes()
            .saturating_sub(std::mem::size_of::<InferredOrderKey>())
            .saturating_add(map_bytes)
            .saturating_add(column.retained_bytes())
            .saturating_add(2 * std::mem::size_of::<usize>())
    }
}

impl SortColumn {
    pub(crate) fn retained_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>()
            .saturating_add(
                self.keys
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SortKey>()),
            )
            .saturating_add(
                self.ascending
                    .capacity()
                    .saturating_mul(std::mem::size_of::<RecordSlot>()),
            )
            .saturating_add(
                self.descending
                    .capacity()
                    .saturating_mul(std::mem::size_of::<RecordSlot>()),
            )
            .saturating_add(
                self.path
                    .segments
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(
                self.path
                    .segments
                    .iter()
                    .fold(0usize, |bytes, name| bytes.saturating_add(name.capacity())),
            )
            .saturating_add(self.locale.capacity())
            .saturating_add(self.collation_version.capacity());
        for key in &self.keys {
            bytes = bytes.saturating_add(match key {
                SortKey::String(value) => value.capacity(),
                SortKey::Id(value) => value.capacity(),
                SortKey::Number(Number::ExactDecimal(value)) => value.digits.capacity(),
                _ => 0,
            });
        }
        bytes
    }

    /// Build only the requested query order from a complete inferred scalar
    /// dictionary. Numeric and Boolean keys use the B-tree order directly;
    /// strings use ICU keys because UTF-8 order need not equal locale order.
    pub(crate) fn from_inferred_posting(
        index: &PostingIndex,
        records: &[impl RecordIdSlot],
        path: FieldPath,
        locale: &str,
        options: SortOptions,
        descending: bool,
        execution: Option<&crate::query_execution::streaming::ExecutionContext>,
    ) -> Result<Option<Self>, String> {
        const MAX_QUERY_ORDER_BYTES: usize = 128 * 1024 * 1024;
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        let limit = execution.map_or(MAX_QUERY_ORDER_BYTES, |execution| {
            execution
                .limits
                .inferred_order_bytes
                .min(MAX_QUERY_ORDER_BYTES)
        });
        let count = records.len();
        let Some(mut estimated_bytes) = count
            .checked_mul(std::mem::size_of::<SortKey>() + 3 * std::mem::size_of::<RecordSlot>())
        else {
            return Ok(None);
        };
        if estimated_bytes > limit {
            if let Some(execution) = execution
                .filter(|execution| execution.limits.inferred_order_bytes < MAX_QUERY_ORDER_BYTES)
            {
                return Err(execution.fail(
                    crate::query_execution::streaming::ExecutionErrorKind::Resource,
                    "inferred order exceeds retained-byte limit",
                ));
            }
            return Ok(None);
        }
        let collator = if index.kind == LogicalType::String {
            Some(make_collator(&SortConfig {
                locale: locale.to_owned(),
                options,
            })?)
        } else {
            None
        };
        let mut keys = records
            .iter()
            .map(|record| {
                if record.record_class() == RecordClass::RawFallback {
                    SortKey::Unavailable
                } else {
                    SortKey::Missing
                }
            })
            .collect::<Vec<_>>();
        let nulls = index.values.get(&ScalarKey::Null);
        for slot in nulls.into_iter().flatten() {
            let key = keys
                .get_mut(slot as usize)
                .ok_or("inferred sort null slot out of range")?;
            if !matches!(key, SortKey::Missing) {
                return Err("inferred sort null overlaps another key".into());
            }
            *key = SortKey::Null;
        }
        let value_map = &index.values;
        for (value, slots) in value_map.iter() {
            if matches!(value, ScalarKey::Null) {
                continue;
            }
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let key = match value {
                ScalarKey::Number(number) if index.kind == LogicalType::Number => {
                    SortKey::Number(number.clone())
                }
                ScalarKey::Bool(flag) if index.kind == LogicalType::Bool => SortKey::Bool(*flag),
                ScalarKey::String(text) if index.kind == LogicalType::String => {
                    let mut bytes = Vec::new();
                    collator
                        .as_ref()
                        .expect("string collator")
                        .write_sort_key_to(text, &mut bytes)
                        .map_err(|error| error.to_string())?;
                    let memberships = usize::try_from(slots.len())
                        .map_err(|_| "inferred sort membership count overflow")?;
                    let key_bytes = bytes
                        .len()
                        .checked_mul(memberships)
                        .ok_or("inferred sort key size overflow")?;
                    estimated_bytes = estimated_bytes
                        .checked_add(key_bytes)
                        .ok_or("inferred sort key size overflow")?;
                    if estimated_bytes > limit {
                        if let Some(execution) = execution.filter(|execution| {
                            execution.limits.inferred_order_bytes < MAX_QUERY_ORDER_BYTES
                        }) {
                            return Err(execution.fail(
                                crate::query_execution::streaming::ExecutionErrorKind::Resource,
                                "inferred order exceeds retained-byte limit",
                            ));
                        }
                        return Ok(None);
                    }
                    SortKey::String(bytes)
                }
                _ => return Err("inferred sort posting type mismatch".into()),
            };
            for slot in slots {
                let target = keys
                    .get_mut(slot as usize)
                    .ok_or("inferred sort value slot out of range")?;
                if !matches!(target, SortKey::Missing) {
                    return Err("inferred sort value overlaps another key".into());
                }
                *target = key.clone();
            }
        }
        if index.presence.iter().any(|slot| {
            keys.get(slot as usize)
                .is_none_or(|key| matches!(key, SortKey::Missing | SortKey::Unavailable))
        }) {
            return Err("inferred sort presence lacks a scalar key".into());
        }
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        let mut order = Vec::with_capacity(count);
        if index.kind == LogicalType::String {
            order.extend(records.iter().map(|record| record.record_slot()));
            order.sort_unstable_by(|a, b| {
                keys[a.0 as usize]
                    .compare_with(&keys[b.0 as usize], descending, options)
                    .then_with(|| {
                        records[a.0 as usize]
                            .record_id()
                            .cmp(records[b.0 as usize].record_id())
                    })
            });
        } else {
            let mut missing = Vec::new();
            let mut nulls = Vec::new();
            for record in records {
                match keys[record.record_slot().0 as usize] {
                    SortKey::Missing | SortKey::Unavailable => missing.push(record.record_slot()),
                    SortKey::Null => nulls.push(record.record_slot()),
                    _ => {}
                }
            }
            let by_id = |a: &RecordSlot, b: &RecordSlot| {
                records[a.0 as usize]
                    .record_id()
                    .cmp(records[b.0 as usize].record_id())
            };
            missing.sort_unstable_by(by_id);
            nulls.sort_unstable_by(by_id);
            if options.nulls == Placement::First {
                order.extend(&nulls);
            }
            if options.missing == Placement::First {
                order.extend(&missing);
            }
            if descending {
                for (_, slots) in value_map
                    .iter()
                    .rev()
                    .filter(|(key, _)| !matches!(key, ScalarKey::Null))
                {
                    append_inferred_group(&mut order, slots, records);
                }
            } else {
                for (_, slots) in value_map
                    .iter()
                    .filter(|(key, _)| !matches!(key, ScalarKey::Null))
                {
                    append_inferred_group(&mut order, slots, records);
                }
            }
            if options.nulls == Placement::Last {
                order.extend(&nulls);
            }
            if options.missing == Placement::Last {
                order.extend(&missing);
            }
        }
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        if order.len() != count {
            return Err("inferred sort order coverage mismatch".into());
        }
        let (ascending, descending_order) = if descending {
            (Vec::new(), order)
        } else {
            (order, Vec::new())
        };
        Ok(Some(Self {
            path,
            kind: index.kind.clone(),
            keys,
            ascending,
            descending: descending_order,
            locale: locale.to_owned(),
            options,
            collation_version: COLLATION_VERSION.into(),
        }))
    }
}

fn append_inferred_group(
    order: &mut Vec<RecordSlot>,
    slots: &RoaringBitmap,
    records: &[impl RecordIdSlot],
) {
    let mut group = slots.iter().map(RecordSlot).collect::<Vec<_>>();
    group.sort_unstable_by(|a, b| {
        records[a.0 as usize]
            .record_id()
            .cmp(records[b.0 as usize].record_id())
    });
    order.extend(group);
}

#[cfg(test)]
#[path = "inferred_order.test.rs"]
mod tests;
