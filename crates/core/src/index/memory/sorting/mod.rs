//! Stored ICU string keys and typed sort columns.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use icu_collator::CollatorBorrowed;
use roaring::RoaringBitmap;

use crate::index::storage::ordered_slots::OrderedSlots;
use crate::index::storage::transfer::{BatchLimit, BatchStatus, StorageError, StorageResult};
use crate::inventory::{ByteRange, CollectionGeneration, FileEntry, FileInventory, RecordClass};
use crate::model::{
    FieldPath, LogicalType, MetadataField, Number, PathRoot, RecordId, RecordSlot, Value,
};
use crate::query::Operator;
use crate::query::options::SortField;
use crate::query::semantics::record::{data_values_small, metadata_value_for_record};
use crate::query_access::membership::MembershipView;
use crate::records::RecordHeader;
use crate::records::RecordIdSlot;
use crate::schema::Schema;

mod inferred_order;
mod placement;

pub(super) use inferred_order::build_inferred_order_rows;

use crate::index::storage::order::declared::{
    composite_orders, order_configuration, validate_overrides,
};
pub(crate) use crate::index::storage::order::simple_composite_part;
pub use crate::index::storage::order::{
    COLLATION_VERSION, CollationStrength, CompositeSortPart, Placement, SortConfig, SortKey,
    SortOptions,
};
use crate::index::storage::order::{
    InferredOrderRows, OrderColumnReader, ScalarOrderPredicate, make_collator,
};

impl OrderedSlots for Vec<RecordSlot> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn read_batch(
        &self,
        start: usize,
        limit: BatchLimit,
    ) -> StorageResult<(Cow<'_, [RecordSlot]>, BatchStatus)> {
        const OP: &str = "read native order";
        let remaining = self
            .get(start..)
            .ok_or_else(|| StorageError::invalid(OP, "slot cursor outside order"))?;
        let len = remaining
            .len()
            .min(limit.fixed_batch_len(OP, std::mem::size_of::<RecordSlot>())?);
        let status = if len == remaining.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((Cow::Borrowed(&remaining[..len]), status))
    }
}

/// Precomputed sort keys for one configured field.
#[derive(Clone, Debug)]
pub struct SortColumn {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub keys: Vec<SortKey>,
    pub ascending: Vec<RecordSlot>,
    pub descending: Vec<RecordSlot>,
    pub locale: String,
    pub options: SortOptions,
    pub collation_version: String,
}

impl OrderColumnReader for SortColumn {
    fn path(&self) -> &FieldPath {
        &self.path
    }

    fn kind(&self) -> &LogicalType {
        &self.kind
    }

    fn collation_version(&self) -> &str {
        &self.collation_version
    }

    fn options(&self) -> SortOptions {
        self.options
    }

    fn locale(&self) -> &str {
        &self.locale
    }

    fn retained_bytes(&self) -> usize {
        SortColumn::retained_bytes(self)
    }

    fn key_count(&self) -> usize {
        self.keys.len()
    }

    fn match_scalar(
        &self,
        predicate: &ScalarOrderPredicate,
        input: MembershipView<'_>,
        ordered_only: bool,
    ) -> StorageResult<Option<RoaringBitmap>> {
        if input
            .last()
            .is_some_and(|last| last as usize >= self.keys.len())
        {
            return Err(StorageError::invalid(
                "filter native sort column",
                "slot outside column",
            ));
        }
        // A typed numeric/temporal range is contiguous in the ascending
        // sequence when the endpoints have the same type as the predicate.
        if matches!(
            predicate.operator,
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) && self.ascending.len() == self.keys.len()
            && let (Some(first), Some(last)) = (
                self.ascending
                    .first()
                    .and_then(|slot| self.keys.get(slot.0 as usize)),
                self.ascending
                    .last()
                    .and_then(|slot| self.keys.get(slot.0 as usize)),
            )
            && std::mem::discriminant(first) == std::mem::discriminant(&predicate.literal)
            && std::mem::discriminant(last) == std::mem::discriminant(&predicate.literal)
        {
            if predicate.matches(first) && predicate.matches(last) {
                let matches = match input {
                    MembershipView::Borrowed(bitmap) | MembershipView::AllSlots(bitmap) => {
                        bitmap.clone()
                    }
                    _ => input.iter().collect(),
                };
                return Ok(Some(matches));
            }
            let lower = self.ascending.partition_point(|slot| {
                self.keys[slot.0 as usize]
                    .compare(&predicate.literal, false)
                    .is_lt()
            });
            let upper = self.ascending.partition_point(|slot| {
                self.keys[slot.0 as usize]
                    .compare(&predicate.literal, false)
                    .is_le()
            });
            let selected = match predicate.operator {
                Operator::Gt => &self.ascending[upper..],
                Operator::Ge => &self.ascending[lower..],
                Operator::Lt => &self.ascending[..lower],
                Operator::Le => &self.ascending[..upper],
                _ => unreachable!(),
            };
            let matches = match input {
                MembershipView::Borrowed(bitmap) | MembershipView::AllSlots(bitmap) => {
                    let mut matches: RoaringBitmap = selected.iter().map(|slot| slot.0).collect();
                    matches &= bitmap;
                    matches
                }
                _ => selected
                    .iter()
                    .filter_map(|slot| input.contains(slot.0).then_some(slot.0))
                    .collect(),
            };
            return Ok(Some(matches));
        }
        if ordered_only {
            return Ok(None);
        }
        Ok(Some(
            input
                .iter()
                .filter(|&slot| predicate.matches(&self.keys[slot as usize]))
                .collect(),
        ))
    }

    fn key(&self, slot: RecordSlot) -> StorageResult<Cow<'_, SortKey>> {
        self.keys
            .get(slot.0 as usize)
            .map(Cow::Borrowed)
            .ok_or_else(|| StorageError::invalid("read native sort key", "slot outside order"))
    }

    fn slots(&self, descending: bool) -> StorageResult<&dyn OrderedSlots> {
        Ok(if descending {
            &self.descending
        } else {
            &self.ascending
        })
    }
}

impl From<InferredOrderRows> for SortColumn {
    fn from(rows: InferredOrderRows) -> Self {
        let (ascending, descending) = if rows.descending {
            (Vec::new(), rows.order)
        } else {
            (rows.order, Vec::new())
        };
        Self {
            path: rows.path,
            kind: rows.kind,
            keys: rows.keys,
            ascending,
            descending,
            locale: rows.config.locale,
            options: rows.config.options,
            collation_version: COLLATION_VERSION.into(),
        }
    }
}

/// All sort columns available for a collection.
#[derive(Clone, Debug)]
pub struct SortCollection {
    pub columns: HashMap<FieldPath, SortColumn>,
    pub composites: Vec<CompositeSortOrder>,
    pub(in crate::index::memory) placed: HashMap<FieldPath, placement::PlacedOrder>,
}

/// Explicit composite order over compatible resident scalar columns.
#[derive(Clone, Debug)]
pub struct CompositeSortOrder {
    pub fields: Vec<CompositeSortPart>,
    pub order: Vec<RecordSlot>,
}

/// Fixed binary identity for one resident composite constituent.
impl SortCollection {
    pub fn composite(&self, fields: &[SortField]) -> Option<&CompositeSortOrder> {
        let normalized = fields
            .iter()
            .map(simple_composite_part)
            .collect::<Option<Vec<_>>>()?;
        self.composites
            .iter()
            .find(|entry| entry.fields == normalized)
    }

    /// Refresh only the metadata columns affected by a changed native file
    /// association. The caller supplies the pinned catalog's file entry.
    pub(crate) fn refresh_metadata_slots<'a>(
        &mut self,
        records: &[RecordHeader],
        slots: impl IntoIterator<Item = RecordSlot>,
        mut origin: impl FnMut(RecordSlot) -> Option<(&'a FileEntry, Option<ByteRange>)>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> Result<(), String> {
        let paths = self
            .columns
            .keys()
            .filter(|path| matches!(path.root, PathRoot::Metadata(_)))
            .cloned()
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(());
        }
        let collators = paths
            .iter()
            .map(|path| {
                let column = &self.columns[path];
                make_collator(&SortConfig {
                    locale: column.locale.clone(),
                    options: column.options,
                })
                .map(|collator| (path.clone(), collator))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut changed = false;
        for slot in slots {
            let record = records
                .get(slot.0 as usize)
                .ok_or("metadata sort slot missing")?;
            let file = origin(slot);
            for path in &paths {
                let PathRoot::Metadata(field) = path.root else {
                    unreachable!()
                };
                let value = metadata_value_for_record(
                    field,
                    &record.id,
                    record.class,
                    file,
                    collection_name,
                    root,
                )?;
                let key = sort_key_from_value(value.as_ref(), &collators[path], path)?;
                let column = self
                    .columns
                    .get_mut(path)
                    .ok_or("metadata sort column missing")?;
                let target = column
                    .keys
                    .get_mut(slot.0 as usize)
                    .ok_or("metadata sort key missing")?;
                *target = key;
            }
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        for path in &paths {
            let column = self
                .columns
                .get_mut(path)
                .ok_or("metadata sort column missing")?;
            column.ascending = records.iter().map(|record| record.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
            column.descending = descending_order(&column.ascending, &column.keys, column.options);
        }
        for composite in &mut self.composites {
            if !composite
                .fields
                .iter()
                .any(|field| paths.contains(&field.path))
            {
                continue;
            }
            let columns = composite
                .fields
                .iter()
                .map(|field| {
                    self.columns
                        .get(&field.path)
                        .map(|column| (column, field.descending))
                        .ok_or("composite metadata column missing")
                })
                .collect::<Result<Vec<_>, _>>()?;
            composite.order = records.iter().map(|record| record.slot).collect();
            composite.order.sort_by(|a, b| {
                compare_composite(&columns, *a, *b)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        }
        Ok(())
    }
}

fn sort_key_from_value(
    value: Option<&Value>,
    collator: &CollatorBorrowed<'_>,
    path: &FieldPath,
) -> Result<SortKey, String> {
    Ok(match value {
        None => SortKey::Missing,
        Some(Value::Null) => SortKey::Null,
        Some(Value::String(text)) => {
            let mut key = Vec::new();
            let Ok(()) = collator.write_sort_key_to(text, &mut key);
            SortKey::String(key)
        }
        Some(Value::Id(id)) => SortKey::Id(id.clone()),
        Some(Value::Number(number)) => SortKey::Number(number.clone()),
        Some(Value::Bool(value)) => SortKey::Bool(*value),
        Some(Value::Date(value)) => SortKey::Date(*value),
        Some(Value::Datetime(value)) => SortKey::Datetime(*value),
        Some(Value::Unixtime(value)) => SortKey::Unixtime(*value),
        _ => return Err(format!("unsupported sort value: {}", path.display())),
    })
}

/// Mutable keys accumulated during index construction.
pub struct SortBuilder {
    columns: HashMap<FieldPath, SortColumn>,
    collators: HashMap<FieldPath, CollatorBorrowed<'static>>,
    composites: Vec<Vec<CompositeSortPart>>,
}

impl SortBuilder {
    pub(crate) fn into_keys(self) -> HashMap<FieldPath, Vec<SortKey>> {
        self.columns
            .into_iter()
            .map(|(path, column)| (path, column.keys))
            .collect()
    }

    pub(crate) fn append_keys(
        &mut self,
        keys: HashMap<FieldPath, Vec<SortKey>>,
        base: u32,
        batch_count: u32,
    ) -> Result<(), String> {
        if self.columns.len() != keys.len() {
            return Err("sort batch column coverage mismatch".into());
        }
        for (path, mut values) in keys {
            let column = self
                .columns
                .get_mut(&path)
                .ok_or("sort batch column missing")?;
            if column.keys.len() != base as usize || values.len() != batch_count as usize {
                return Err("sort batch key coverage mismatch".into());
            }
            column.keys.append(&mut values);
        }
        Ok(())
    }

    pub fn new(
        paths: &[FieldPath],
        schema: &Schema,
        locale: &str,
        defaults: SortOptions,
        overrides: &HashMap<FieldPath, SortConfig>,
    ) -> Result<Self, String> {
        let mut columns = HashMap::new();
        let mut collators = HashMap::new();
        validate_overrides(paths, overrides)?;
        for path in paths {
            let (config, kind, collator) =
                order_configuration(path, schema, locale, defaults, overrides)?;
            collators.insert(path.clone(), collator);
            if columns
                .insert(
                    path.clone(),
                    SortColumn {
                        path: path.clone(),
                        kind,
                        keys: Vec::new(),
                        ascending: Vec::new(),
                        descending: Vec::new(),
                        locale: config.locale,
                        options: config.options,
                        collation_version: COLLATION_VERSION.into(),
                    },
                )
                .is_some()
            {
                return Err("duplicate sort field".into());
            }
        }
        let composites = composite_orders(schema, |path| columns.contains_key(path))?;
        Ok(Self {
            columns,
            collators,
            composites,
        })
    }

    pub fn add(
        &mut self,
        slot: RecordSlot,
        record: &Value,
        id: &RecordId,
        class: RecordClass,
        file: Option<(&FileEntry, Option<ByteRange>)>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> Result<(), String> {
        for column in self.columns.values_mut() {
            if column.keys.len() != slot.0 as usize {
                return Err("sort slots out of order".into());
            }
            let owned;
            let selected = match column.path.root {
                PathRoot::Metadata(field) => {
                    owned =
                        metadata_value_for_record(field, id, class, file, collection_name, root)?;
                    owned.as_ref().map(|value| smallvec::smallvec![value])
                }
                PathRoot::Data => data_values_small(record, &column.path.segments),
                PathRoot::Backlinks => return Err("related sort field is not registered".into()),
            };
            let key = match selected {
                Some(values) if values.len() > 1 => {
                    return Err(format!(
                        "multi-valued sort field: {}",
                        column.path.display()
                    ));
                }
                values => sort_key_from_value(
                    values.and_then(|values| values.first().copied()),
                    &self.collators[&column.path],
                    &column.path,
                )?,
            };
            column.keys.push(key);
        }
        Ok(())
    }

    pub fn finish(self, generation: &CollectionGeneration) -> Result<SortCollection, String> {
        self.finish_for_records(&generation.records)
    }

    /// Finalize a locally slotted incremental segment without constructing
    /// the dense per-file ownership table required by a base generation.
    pub(crate) fn finish_for_records(
        mut self,
        records: &[impl RecordIdSlot],
    ) -> Result<SortCollection, String> {
        for column in self.columns.values_mut() {
            column.build_order(records)?;
        }
        let composites = self
            .composites
            .into_iter()
            .map(|fields| {
                let columns: Vec<_> = fields
                    .iter()
                    .map(|field| (&self.columns[&field.path], field.descending))
                    .collect();
                let mut order: Vec<_> = records.iter().map(|record| record.record_slot()).collect();
                order.sort_by(|a, b| {
                    compare_composite(&columns, *a, *b).then_with(|| {
                        records[a.0 as usize]
                            .record_id()
                            .cmp(records[b.0 as usize].record_id())
                    })
                });
                CompositeSortOrder { fields, order }
            })
            .collect();
        Ok(SortCollection {
            columns: self.columns,
            composites,
            placed: HashMap::new(),
        })
    }
}

impl SortColumn {
    pub(in crate::index::memory) fn build_order(
        &mut self,
        records: &[impl RecordIdSlot],
    ) -> Result<(), String> {
        if self.keys.len() != records.len() {
            return Err("sort column coverage mismatch".into());
        }
        self.ascending = records.iter().map(|r| r.record_slot()).collect();
        self.ascending.sort_by(|a, b| {
            self.keys[a.0 as usize]
                .compare_with(&self.keys[b.0 as usize], false, self.options)
                .then_with(|| {
                    records[a.0 as usize]
                        .record_id()
                        .cmp(records[b.0 as usize].record_id())
                })
        });
        self.descending = descending_order(&self.ascending, &self.keys, self.options);
        Ok(())
    }
}

pub(crate) fn compare_composite(
    columns: &[(&SortColumn, bool)],
    a: RecordSlot,
    b: RecordSlot,
) -> Ordering {
    for (column, descending) in columns {
        let order = column.keys[a.0 as usize].compare_with(
            &column.keys[b.0 as usize],
            *descending,
            column.options,
        );
        if !order.is_eq() {
            return order;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod neutral_metadata_tests {
    use super::*;

    #[test]
    fn unlinked_record_has_missing_file_sort_key_and_record_identity() {
        let file_path = FieldPath::parse("@file.id").unwrap();
        let id_path = FieldPath::parse("@record.id").unwrap();
        let mut builder = SortBuilder::new(
            &[file_path.clone(), id_path.clone()],
            &Schema::default(),
            "und",
            SortOptions::default(),
            &HashMap::new(),
        )
        .unwrap();
        let id = RecordId::intrinsic("entry-47").unwrap();
        builder
            .add(
                RecordSlot(0),
                &Value::Null,
                &id,
                RecordClass::Logical,
                None,
                None,
                "entries",
            )
            .unwrap();
        let keys = builder.into_keys();
        assert!(matches!(keys[&file_path][0], SortKey::Missing));
        assert!(matches!(&keys[&id_path][0], SortKey::Id(value) if value == "entry-47"));
    }
}

fn descending_order(
    ascending: &[RecordSlot],
    keys: &[SortKey],
    options: SortOptions,
) -> Vec<RecordSlot> {
    let mut result = Vec::with_capacity(ascending.len());
    let mut start = 0;
    while start < ascending.len() {
        let rank = keys[ascending[start].0 as usize].rank(options);
        let mut end = start + 1;
        while end < ascending.len() && keys[ascending[end].0 as usize].rank(options) == rank {
            end += 1;
        }
        if rank == 0 {
            let mut group_end = end;
            while group_end > start {
                let mut group_start = group_end - 1;
                while group_start > start
                    && keys[ascending[group_start - 1].0 as usize]
                        .compare_with(&keys[ascending[group_end - 1].0 as usize], false, options)
                        .is_eq()
                {
                    group_start -= 1;
                }
                result.extend_from_slice(&ascending[group_start..group_end]);
                group_end = group_start;
            }
        } else {
            result.extend_from_slice(&ascending[start..end]);
        }
        start = end;
    }
    result
}

impl SortCollection {
    /// Finalize status-dependent orders after all collections have contributed
    /// diagnostics to the shared file inventory.
    pub fn refresh_file_status(
        &mut self,
        generation: &CollectionGeneration,
        inventory: &FileInventory,
    ) {
        self.refresh_file_status_for_records(&generation.records, |slot| {
            let record = &generation.records[slot.0 as usize];
            inventory
                .files
                .get(record.file.0 as usize)
                .map(|file| file.status.code())
        });
    }

    /// Finalize status-dependent orders from a segment's local file table.
    pub(crate) fn refresh_file_status_for_records(
        &mut self,
        records: &[impl crate::records::RecordIdSlot],
        status: impl Fn(RecordSlot) -> Option<u8>,
    ) {
        let mut refreshed = false;
        for column in self.columns.values_mut() {
            if column.path.root != PathRoot::Metadata(MetadataField::FileStatus) {
                continue;
            }
            refreshed = true;
            for record in records {
                column.keys[record.record_slot().0 as usize] = status(record.record_slot())
                    .map_or(SortKey::Missing, |code| {
                        SortKey::Number(Number::integer(code as i128))
                    });
            }
            column.ascending = records.iter().map(|r| r.record_slot()).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| {
                        records[a.0 as usize]
                            .record_id()
                            .cmp(records[b.0 as usize].record_id())
                    })
            });
            column.descending = descending_order(&column.ascending, &column.keys, column.options);
        }
        if refreshed {
            for composite in &mut self.composites {
                if !composite
                    .fields
                    .iter()
                    .any(|field| field.path.root == PathRoot::Metadata(MetadataField::FileStatus))
                {
                    continue;
                }
                let columns = composite
                    .fields
                    .iter()
                    .map(|field| (&self.columns[&field.path], field.descending))
                    .collect::<Vec<_>>();
                composite.order.sort_by(|a, b| {
                    compare_composite(&columns, *a, *b).then_with(|| {
                        records[a.0 as usize]
                            .record_id()
                            .cmp(records[b.0 as usize].record_id())
                    })
                });
            }
        }
    }

    pub fn column(&self, field: &str) -> Result<&SortColumn, String> {
        let path = FieldPath::parse(field)?;
        self.columns
            .get(&path)
            .ok_or_else(|| format!("sort field is not registered: {field}"))
    }

    pub fn key(&self, field: &str, slot: RecordSlot) -> Result<&SortKey, String> {
        self.column(field)?
            .keys
            .get(slot.0 as usize)
            .ok_or("sort slot out of bounds".into())
    }

    pub fn validate_request(&self, fields: &[SortField]) -> Result<(), String> {
        for field in fields {
            self.key(&field.field, RecordSlot(0)).or_else(|e| {
                // An empty collection has no slot zero; the column still exists.
                if self.columns.contains_key(&FieldPath::parse(&field.field)?) {
                    Ok(&SortKey::Missing)
                } else {
                    Err(e)
                }
            })?;
        }
        Ok(())
    }
}
