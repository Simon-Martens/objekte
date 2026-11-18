use std::collections::HashMap;

use super::super::IndexedCollectionSegment;
use super::routing::{PartitionRouting, SlotMap};
use crate::index::sorting::{CompositeSortOrder, SortCollection, SortColumn, compare_composite};
use crate::inventory::RecordEntry;

pub(crate) fn project_sorting(
    source: &SortCollection,
    slots: &SlotMap<'_>,
    records: &[RecordEntry],
) -> Result<SortCollection, String> {
    let mut sorting = SortCollection {
        columns: HashMap::with_capacity(source.columns.len()),
        composites: source
            .composites
            .iter()
            .map(|order| CompositeSortOrder {
                fields: order.fields.clone(),
                order: Vec::new(),
            })
            .collect(),
    };
    for (path, original) in &source.columns {
        let mut column = SortColumn {
            path: original.path.clone(),
            kind: original.kind.clone(),
            keys: Vec::new(),
            ascending: Vec::new(),
            descending: Vec::new(),
            locale: original.locale.clone(),
            options: original.options,
            collation_version: original.collation_version.clone(),
        };
        let old_keys = &original.keys;
        if old_keys.len() != slots.len() {
            return Err("sort projection coverage mismatch".into());
        }
        let mut keys = std::iter::repeat_with(|| None)
            .take(records.len())
            .collect::<Vec<_>>();
        slots.for_each_selected(|old, slot| {
            let key = old_keys
                .get(old)
                .ok_or("sort projection slot out of bounds")?;
            let target = keys
                .get_mut(slot.0 as usize)
                .ok_or("sort projection slot out of bounds")?;
            if target.replace(key.clone()).is_some() {
                return Err("duplicate sort projection slot".into());
            }
            Ok(())
        })?;
        column.keys = keys
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("sort projection slot mismatch")?;
        column.ascending = records.iter().map(|record| record.slot).collect();
        column.ascending.sort_by(|a, b| {
            column.keys[a.0 as usize]
                .compare_with(&column.keys[b.0 as usize], false, column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
        column.descending = records.iter().map(|record| record.slot).collect();
        column.descending.sort_by(|a, b| {
            column.keys[a.0 as usize]
                .compare_with(&column.keys[b.0 as usize], true, column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
        sorting.columns.insert(path.clone(), column);
    }
    for composite in &mut sorting.composites {
        let columns = composite
            .fields
            .iter()
            .map(|part| {
                sorting
                    .columns
                    .get(&part.path)
                    .map(|column| (column, part.descending))
                    .ok_or("composite sort column missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        composite.order = records.iter().map(|record| record.slot).collect();
        composite.order.sort_by(|a, b| {
            compare_composite(&columns, *a, *b)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
    }
    Ok(sorting)
}

impl IndexedCollectionSegment {
    pub(crate) fn project_sorting_for_partitions(
        &self,
        routing: &PartitionRouting,
    ) -> Result<Vec<SortCollection>, String> {
        let source_len = self.record_count();
        let mut parts = routing
            .selected
            .iter()
            .map(|_| SortCollection {
                columns: HashMap::with_capacity(self.sorting.columns.len()),
                composites: self
                    .sorting
                    .composites
                    .iter()
                    .map(|order| CompositeSortOrder {
                        fields: order.fields.clone(),
                        order: Vec::new(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        for (path, original) in &self.sorting.columns {
            if original.keys.len() != source_len
                || original.ascending.len() != source_len
                || original.descending.len() != source_len
            {
                return Err("sort projection coverage mismatch".into());
            }
            for (part, selected) in routing.selected.iter().enumerate() {
                let keys = selected
                    .iter()
                    .map(|&old| {
                        original
                            .keys
                            .get(old as usize)
                            .cloned()
                            .ok_or("sort projection slot out of bounds")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                parts[part].columns.insert(
                    path.clone(),
                    SortColumn {
                        path: original.path.clone(),
                        kind: original.kind.clone(),
                        keys,
                        ascending: Vec::with_capacity(selected.len()),
                        descending: Vec::with_capacity(selected.len()),
                        locale: original.locale.clone(),
                        options: original.options,
                        collation_version: original.collation_version.clone(),
                    },
                );
            }
            for (source_order, descending) in
                [(&original.ascending, false), (&original.descending, true)]
            {
                for old in source_order {
                    if let Some((part, slot)) = routing
                        .locations
                        .get(old.0 as usize)
                        .ok_or("sort order slot outside source")?
                    {
                        let column = parts[*part]
                            .columns
                            .get_mut(path)
                            .ok_or("partition sort column missing")?;
                        if descending {
                            column.descending.push(*slot);
                        } else {
                            column.ascending.push(*slot);
                        }
                    }
                }
            }
        }
        for (number, original) in self.sorting.composites.iter().enumerate() {
            if original.order.len() != source_len
                || original
                    .fields
                    .iter()
                    .any(|field| !self.sorting.columns.contains_key(&field.path))
            {
                return Err("composite sort projection coverage mismatch".into());
            }
            for old in &original.order {
                if let Some((part, slot)) = routing
                    .locations
                    .get(old.0 as usize)
                    .ok_or("composite sort slot outside source")?
                {
                    parts[*part].composites[number].order.push(*slot);
                }
            }
        }
        for (part, sorting) in parts.iter().enumerate() {
            let expected = routing.selected[part].len();
            if sorting.columns.values().any(|column| {
                column.keys.len() != expected
                    || column.ascending.len() != expected
                    || column.descending.len() != expected
            }) || sorting
                .composites
                .iter()
                .any(|order| order.order.len() != expected)
            {
                return Err("partition sort order coverage mismatch".into());
            }
        }
        Ok(parts)
    }
}
