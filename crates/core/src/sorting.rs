//! Stored ICU string keys and typed sort columns.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

use icu_collator::{
    CollatorBorrowed, CollatorPreferences,
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
};
use icu_locale_core::Locale;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::inventory::{CollectionGeneration, FileEntry, FileInventory, RecordClass, RecordEntry};
use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, RecordSlot, Value};
use crate::posting::{PostingIndex, ScalarKey};
use crate::reference::{RecordContext, Schema, data_values_small, metadata_value};
use crate::workload::SortField;

pub const COLLATION_VERSION: &str =
    "icu_collator-2.4.0/icu_collator_data-2.4.0/CLDR-49.0.0/Unicode-18.0.0";
pub(crate) const INFERRED_ORDER_CACHE_BYTES: usize = 128 * 1024 * 1024;


#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum CollationStrength {
    #[default]
    LocaleDefault,
    Primary,
    Secondary,
    Tertiary,
    Quaternary,
    Identical,
}


#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Placement {
    First,
    #[default]
    Last,
}


#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SortOptions {
    pub strength: CollationStrength,
    pub numeric: bool,
    pub nulls: Placement,
    pub missing: Placement,
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortConfig {
    pub locale: String,
    pub options: SortOptions,
}


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortKey {
    Missing,
    /// A reached value whose logical payload is unavailable, ordered with missing.
    Unavailable,
    Null,
    String(Vec<u8>),
    Id(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
}


impl SortKey {
    fn rank(&self, options: SortOptions) -> i8 {
        match self {
            Self::Missing | Self::Unavailable if options.missing == Placement::First => -1,
            Self::Missing | Self::Unavailable => 2,
            Self::Null if options.nulls == Placement::First => -2,
            Self::Null => 1,
            _ => 0,
        }
    }

    pub fn compare(&self, other: &Self, descending: bool) -> Ordering {
        self.compare_with(other, descending, SortOptions::default())
    }

    pub fn compare_with(&self, other: &Self, descending: bool, options: SortOptions) -> Ordering {
        let rank = self.rank(options).cmp(&other.rank(options));
        if !rank.is_eq() {
            return rank;
        }
        let order = match (self, other) {
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (Self::Id(a), Self::Id(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Date(a), Self::Date(b)) => a.cmp(b),
            (Self::Datetime(a), Self::Datetime(b)) => a.cmp(b),
            (Self::Unixtime(a), Self::Unixtime(b)) => a.cmp(b),
            (Self::Bool(_), _) => Ordering::Less,
            (_, Self::Bool(_)) => Ordering::Greater,
            (Self::Number(_), _) => Ordering::Less,
            (_, Self::Number(_)) => Ordering::Greater,
            (Self::Date(_), _) => Ordering::Less,
            (_, Self::Date(_)) => Ordering::Greater,
            (Self::Datetime(_), _) => Ordering::Less,
            (_, Self::Datetime(_)) => Ordering::Greater,
            (Self::Unixtime(_), _) => Ordering::Less,
            (_, Self::Unixtime(_)) => Ordering::Greater,
            (Self::Id(_), _) => Ordering::Less,
            (_, Self::Id(_)) => Ordering::Greater,
            (Self::String(_), _) => Ordering::Greater,
            (_, Self::String(_)) => Ordering::Less,
            _ => Ordering::Equal,
        };
        if descending && self.rank(options) == 0 {
            order.reverse()
        } else {
            order
        }
    }
}


/// Precomputed sort keys for one configured field.
#[derive(Clone, Debug, Serialize, Deserialize)]
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


/// An inferred order is query-created but remains valid for a repository
/// generation. Cached columns use the same representation as configured
/// columns, so paging and tie-breaking follow the existing execution path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InferredOrderKey {
    collection: usize,
    generation: u64,
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
        path: FieldPath,
        locale: &str,
        options: SortOptions,
        descending: bool,
    ) -> Self {
        Self {
            collection,
            generation,
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
        generation: &CollectionGeneration,
        path: FieldPath,
        locale: &str,
        options: SortOptions,
        descending: bool,
        execution: Option<&crate::streaming::ExecutionContext>,
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
        let count = generation.records.len();
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
                    crate::streaming::ExecutionErrorKind::Resource,
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
        let mut keys = generation
            .records
            .iter()
            .map(|record| {
                if record.class == RecordClass::RawFallback {
                    SortKey::Unavailable
                } else {
                    SortKey::Missing
                }
            })
            .collect::<Vec<_>>();
        for slot in &index.nulls {
            let key = keys
                .get_mut(slot as usize)
                .ok_or("inferred sort null slot out of range")?;
            if !matches!(key, SortKey::Missing) {
                return Err("inferred sort null overlaps another key".into());
            }
            *key = SortKey::Null;
        }
        for (value, slots) in &index.values {
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
                                crate::streaming::ExecutionErrorKind::Resource,
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
            order.extend(generation.records.iter().map(|record| record.slot));
            order.sort_unstable_by(|a, b| {
                keys[a.0 as usize]
                    .compare_with(&keys[b.0 as usize], descending, options)
                    .then_with(|| {
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
                    })
            });
        } else {
            let mut missing = Vec::new();
            let mut nulls = Vec::new();
            for record in &generation.records {
                match keys[record.slot.0 as usize] {
                    SortKey::Missing | SortKey::Unavailable => missing.push(record.slot),
                    SortKey::Null => nulls.push(record.slot),
                    _ => {}
                }
            }
            let by_id = |a: &RecordSlot, b: &RecordSlot| {
                generation.records[a.0 as usize]
                    .id
                    .cmp(&generation.records[b.0 as usize].id)
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
                for (_, slots) in index.values.iter().rev() {
                    append_inferred_group(&mut order, slots, generation);
                }
            } else {
                for slots in index.values.values() {
                    append_inferred_group(&mut order, slots, generation);
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
    generation: &CollectionGeneration,
) {
    let mut group = slots.iter().map(RecordSlot).collect::<Vec<_>>();
    group.sort_unstable_by(|a, b| {
        generation.records[a.0 as usize]
            .id
            .cmp(&generation.records[b.0 as usize].id)
    });
    order.extend(group);
}


/// All sort columns available for a collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SortCollection {
    pub columns: HashMap<FieldPath, SortColumn>,
    pub composites: Vec<CompositeSortOrder>,
}


/// Explicit composite order over compatible resident scalar columns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompositeSortOrder {
    pub fields: Vec<CompositeSortPart>,
    pub order: Vec<RecordSlot>,
}


/// Fixed binary identity for one resident composite constituent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompositeSortPart {
    pub path: FieldPath,
    pub descending: bool,
}


pub(crate) fn simple_composite_part(field: &SortField) -> Option<CompositeSortPart> {
    let expression = field.expression.as_ref();
    if expression.is_some_and(|expr| expr.binding.is_some() || expr.reducer.is_some()) {
        return None;
    }
    Some(CompositeSortPart {
        path: FieldPath::parse(expression.map_or(field.field.as_str(), |expr| expr.path.as_str()))
            .ok()?,
        descending: field.descending,
    })
}


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
}


/// Mutable keys accumulated during index construction.
pub struct SortBuilder {
    columns: HashMap<FieldPath, SortColumn>,
    collators: HashMap<FieldPath, CollatorBorrowed<'static>>,
    composites: Vec<Vec<CompositeSortPart>>,
}


/// Construct one ICU collator for a bound sort configuration.
pub(crate) fn make_collator(config: &SortConfig) -> Result<CollatorBorrowed<'static>, String> {
    let parsed: Locale = config
        .locale
        .parse()
        .map_err(|e| format!("invalid sort locale: {e}"))?;
    let mut prefs: CollatorPreferences = parsed.into();
    prefs.numeric_ordering = Some(if config.options.numeric {
        CollationNumericOrdering::True
    } else {
        CollationNumericOrdering::False
    });
    let mut options = CollatorOptions::default();
    options.strength = match config.options.strength {
        CollationStrength::LocaleDefault => None,
        CollationStrength::Primary => Some(Strength::Primary),
        CollationStrength::Secondary => Some(Strength::Secondary),
        CollationStrength::Tertiary => Some(Strength::Tertiary),
        CollationStrength::Quaternary => Some(Strength::Quaternary),
        CollationStrength::Identical => Some(Strength::Identical),
    };
    CollatorBorrowed::try_new(prefs, options).map_err(|e| e.to_string())
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
        if overrides.keys().any(|path| !paths.contains(path)) {
            return Err("sort override is not a configured sort field".into());
        }
        for path in paths {
            let config = overrides.get(path).cloned().unwrap_or_else(|| SortConfig {
                locale: locale.into(),
                options: defaults,
            });
            let collator = make_collator(&config)?;
            collators.insert(path.clone(), collator);
            let field = schema.field(path)?;
            if field.multi || matches!(field.kind, LogicalType::Object | LogicalType::Array(_)) {
                return Err(format!(
                    "array/object sort needs explicit reduction: {}",
                    path.display()
                ));
            }
            if columns
                .insert(
                    path.clone(),
                    SortColumn {
                        path: path.clone(),
                        kind: field.kind,
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
        let mut composites = Vec::new();
        for fields in &schema.composite_sorts {
            let parts = fields
                .iter()
                .map(simple_composite_part)
                .collect::<Option<Vec<_>>>();
            if fields.len() < 2
                || parts
                    .as_ref()
                    .is_none_or(|parts| parts.iter().any(|part| !columns.contains_key(&part.path)))
            {
                return Err("composite sort requires configured resident scalar fields".into());
            }
            composites.push(parts.unwrap());
        }
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
        entry: &RecordEntry,
        file: &FileEntry,
        root: &std::path::Path,
        collection_name: &str,
    ) -> Result<(), String> {
        for column in self.columns.values_mut() {
            if column.keys.len() != slot.0 as usize {
                return Err("sort slots out of order".into());
            }
            let owned;
            let selected = match column.path.root {
                PathRoot::Metadata(field) => {
                    let ctx = RecordContext {
                        value: record,
                        source_bytes: None,
                        record: entry,
                        file,
                        collection_name,
                        repository_root: root,
                    };
                    owned = metadata_value(field, &ctx)?;
                    Some(smallvec::smallvec![&owned])
                }
                PathRoot::Data => data_values_small(record, &column.path.segments),
                PathRoot::Backlinks => return Err("related sort field is not registered".into()),
            };
            let key = match selected {
                None => SortKey::Missing,
                Some(values) if values.is_empty() => SortKey::Missing,
                Some(values) if values.len() > 1 => {
                    return Err(format!(
                        "multi-valued sort field: {}",
                        column.path.display()
                    ));
                }
                Some(values) => match values[0] {
                    Value::Null => SortKey::Null,
                    Value::String(text) => {
                        let mut key = Vec::new();
                        self.collators[&column.path]
                            .write_sort_key_to(text, &mut key)
                            .map_err(|e| e.to_string())?;
                        SortKey::String(key)
                    }
                    Value::Id(id) => SortKey::Id(id.clone()),
                    Value::Number(n) => SortKey::Number(n.clone()),
                    Value::Bool(b) => SortKey::Bool(*b),
                    Value::Date(d) => SortKey::Date(*d),
                    Value::Datetime(t) => SortKey::Datetime(*t),
                    Value::Unixtime(t) => SortKey::Unixtime(*t),
                    _ => return Err(format!("unsupported sort value: {}", column.path.display())),
                },
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
        records: &[RecordEntry],
    ) -> Result<SortCollection, String> {
        for column in self.columns.values_mut() {
            if column.keys.len() != records.len() {
                return Err("sort column coverage mismatch".into());
            }
            column.ascending = records.iter().map(|r| r.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
            column.descending = descending_order(&column.ascending, &column.keys, column.options);
        }
        let composites = self
            .composites
            .into_iter()
            .map(|fields| {
                let columns: Vec<_> = fields
                    .iter()
                    .map(|field| (&self.columns[&field.path], field.descending))
                    .collect();
                let mut order: Vec<_> = records.iter().map(|record| record.slot).collect();
                order.sort_by(|a, b| {
                    compare_composite(&columns, *a, *b)
                        .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
                });
                CompositeSortOrder { fields, order }
            })
            .collect();
        Ok(SortCollection {
            columns: self.columns,
            composites,
        })
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
        let mut refreshed = false;
        for column in self.columns.values_mut() {
            if column.path.root != PathRoot::Metadata(MetadataField::FileStatus) {
                continue;
            }
            refreshed = true;
            for record in &generation.records {
                let status = inventory.files[record.file.0 as usize].status.code();
                column.keys[record.slot.0 as usize] =
                    SortKey::Number(Number::integer(status as i128));
            }
            column.ascending = generation.records.iter().map(|r| r.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| {
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
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
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
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


#[cfg(test)]
#[path = "sorting.test.rs"]
mod tests;
