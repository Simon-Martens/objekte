//! Adopt declared order columns from another segment representation.

use std::collections::HashMap;
use std::path::Path;

use crate::index::storage::order::key_transfer::transfer_order_keys;
use crate::index::storage::order::{
    COLLATION_VERSION, CompositeSortPart, OrderColumnReader, SortConfig, SortKey, make_collator,
};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::model::{LogicalType, MetadataField, PathRoot, RecordSlot, Value};
use crate::records::RecordHeader;

use super::{
    CompositeSortOrder, SortCollection, SortColumn, compare_composite, sort_key_from_value,
};

const OP: &str = "import declared order";

pub(in crate::index::memory) fn transfer<'a>(
    readers: Vec<&dyn OrderColumnReader>,
    composites: Vec<&[CompositeSortPart]>,
    mapping: &[Option<RecordSlot>],
    records: &[RecordHeader],
    relocated_path: impl Fn(RecordSlot) -> Option<&'a Path>,
    cancelled: &dyn Fn() -> bool,
) -> StorageResult<SortCollection> {
    let mut columns = HashMap::new();
    for reader in readers {
        if reader.key_count() != mapping.len() || reader.collation_version() != COLLATION_VERSION {
            return Err(StorageError::invalid(
                OP,
                "order coverage or collation version mismatch",
            ));
        }
        let path = reader.path();
        let mut column = SortColumn {
            path: path.clone(),
            kind: reader.kind().clone(),
            keys: Vec::with_capacity(records.len()),
            ascending: Vec::new(),
            descending: Vec::new(),
            locale: reader.locale().to_owned(),
            options: reader.options(),
            collation_version: reader.collation_version().to_owned(),
        };
        let config = SortConfig {
            locale: column.locale.clone(),
            options: column.options,
        };
        let collator = make_collator(&config).map_err(|error| StorageError::invalid(OP, error))?;
        let selected = mapping.iter().enumerate().filter_map(|(old, slot)| {
            slot.map(|_| {
                u32::try_from(old)
                    .map(RecordSlot)
                    .map_err(|_| StorageError::invalid(OP, "source slot overflow"))
            })
        });
        transfer_order_keys(reader, selected, cancelled, |mut key| {
            let slot = records
                .get(column.keys.len())
                .ok_or_else(|| StorageError::invalid(OP, "too many destination keys"))?
                .slot;
            if path.root == PathRoot::Metadata(MetadataField::FilePath)
                && let Some(pathname) = relocated_path(slot)
            {
                let value = Value::String(crate::model::path_representation::query_path(pathname));
                key = sort_key_from_value(Some(&value), &collator, path)
                    .map_err(|error| StorageError::invalid(OP, error))?;
            }
            if !compatible(&column.kind, &key) {
                return Err(StorageError::invalid(
                    OP,
                    "key type does not match the declared order",
                ));
            }
            column.keys.push(key);
            Ok(())
        })?;
        column
            .build_order(records)
            .map_err(|error| StorageError::invalid(OP, error))?;
        if columns.insert(path.clone(), column).is_some() {
            return Err(StorageError::invalid(OP, "duplicate column"));
        }
    }

    let mut sorting = SortCollection {
        columns,
        placed: HashMap::new(),
        composites: Vec::new(),
    };
    for fields in composites {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        if fields.len() < 2
            || sorting
                .composites
                .iter()
                .any(|order| order.fields == fields)
        {
            return Err(StorageError::invalid(
                OP,
                "invalid or duplicate composite order",
            ));
        }
        let columns = fields
            .iter()
            .map(|part| {
                sorting
                    .columns
                    .get(&part.path)
                    .map(|column| (column, part.descending))
                    .ok_or_else(|| StorageError::invalid(OP, "composite column missing"))
            })
            .collect::<StorageResult<Vec<_>>>()?;
        let mut order: Vec<_> = records.iter().map(|record| record.slot).collect();
        order.sort_by(|left, right| {
            compare_composite(&columns, *left, *right).then_with(|| {
                records[left.0 as usize]
                    .id
                    .cmp(&records[right.0 as usize].id)
            })
        });
        sorting.composites.push(CompositeSortOrder {
            fields: fields.to_vec(),
            order,
        });
    }
    Ok(sorting)
}

fn compatible(kind: &LogicalType, key: &SortKey) -> bool {
    use LogicalType as Type;

    matches!(
        (kind, key),
        (_, SortKey::Missing | SortKey::Unavailable | SortKey::Null)
            | (Type::Unknown, _)
            | (Type::String, SortKey::String(_))
            | (Type::Id, SortKey::Id(_))
            | (Type::Number, SortKey::Number(_))
            | (Type::Bool, SortKey::Bool(_))
            | (Type::Date, SortKey::Date(_))
            | (Type::Datetime, SortKey::Datetime(_))
            | (Type::Unixtime, SortKey::Unixtime(_))
    )
}
