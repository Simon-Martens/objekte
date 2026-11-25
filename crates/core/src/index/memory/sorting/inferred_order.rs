use roaring::RoaringBitmap;

use super::{Placement, SortColumn, SortConfig, SortKey, SortOptions, make_collator};
use crate::index::posting::{PostingIndex, ScalarKey};
use crate::index::storage::order::InferredOrderRows;
use crate::inventory::RecordClass;
use crate::model::{FieldPath, LogicalType, Number, RecordSlot};
use crate::records::RecordIdSlot;

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
}

/// Build only the requested query order from a complete inferred scalar
/// dictionary. Numeric and Boolean keys use the B-tree order directly;
/// strings use ICU keys because UTF-8 order need not equal locale order.
pub(crate) fn build_inferred_order_rows(
    index: &PostingIndex,
    records: &[impl RecordIdSlot],
    path: FieldPath,
    locale: &str,
    options: SortOptions,
    descending: bool,
    execution: Option<&crate::query_execution::streaming::ExecutionContext>,
) -> Result<Option<InferredOrderRows>, String> {
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
    let Some(mut estimated_bytes) =
        count.checked_mul(std::mem::size_of::<SortKey>() + 3 * std::mem::size_of::<RecordSlot>())
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
                let Ok(()) = collator
                    .as_ref()
                    .expect("string collator")
                    .write_sort_key_to(text, &mut bytes);
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
    Ok(Some(InferredOrderRows {
        path,
        kind: index.kind.clone(),
        config: SortConfig {
            locale: locale.to_owned(),
            options,
        },
        keys,
        order,
        descending,
    }))
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
