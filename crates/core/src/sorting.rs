//! Stored ICU string keys and typed sort columns.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use icu_collator::{
    CollatorBorrowed, CollatorPreferences,
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
};
use icu_locale_core::Locale;
use rkyv::{Archive, Deserialize as ArchiveDeserialize, Serialize as ArchiveSerialize};
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
    #[serde(default)]
    pub(crate) stored: Option<StoredSortColumnRef>,
    #[serde(skip)]
    pub(crate) loader: Option<Arc<dyn SortColumnLoader>>,
    #[serde(skip)]
    pub(crate) loaded: Arc<OnceLock<Box<SortColumn>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredSortColumnRef {
    pub digest: [u8; 32],
    pub length: u64,
}

pub(crate) trait SortColumnLoader: std::fmt::Debug + Send + Sync {
    fn load_sort_column(&self) -> Result<StoredSortColumn, String>;
}

/// An inferred order is query-created but remains valid for a repository
/// generation. Cached columns use the same representation as configured
/// columns, so paging and tie-breaking follow the existing execution path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InferredOrderKey {
    collection: usize,
    generation: u64,
    segment: crate::native_segment::SegmentId,
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
        segment: crate::native_segment::SegmentId,
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

#[derive(Clone)]
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
#[derive(Clone)]
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
    pub(crate) fn archive_payload(&self) -> Result<StoredSortColumn, String> {
        Ok(StoredSortColumn {
            path: self.path.display(),
            kind: encode_logical_type(&self.kind),
            keys: self.keys.iter().map(encode_sort_key).collect(),
            ascending: self.ascending.iter().map(|slot| slot.0).collect(),
            descending: self.descending.iter().map(|slot| slot.0).collect(),
            locale: self.locale.clone(),
            strength: encode_strength(self.options.strength),
            numeric: self.options.numeric,
            nulls: encode_placement(self.options.nulls),
            missing: encode_placement(self.options.missing),
            collation_version: self.collation_version.clone(),
        })
    }

    pub(crate) fn from_archive_payload(payload: StoredSortColumn) -> Result<Self, String> {
        Ok(Self {
            path: FieldPath::parse(&payload.path)?,
            kind: decode_logical_type(&payload.kind)?,
            keys: payload
                .keys
                .into_iter()
                .map(decode_sort_key)
                .collect::<Result<_, String>>()?,
            ascending: payload.ascending.into_iter().map(RecordSlot).collect(),
            descending: payload.descending.into_iter().map(RecordSlot).collect(),
            locale: payload.locale,
            options: SortOptions {
                strength: decode_strength(payload.strength)?,
                numeric: payload.numeric,
                nulls: decode_placement(payload.nulls)?,
                missing: decode_placement(payload.missing)?,
            },
            collation_version: payload.collation_version,
            stored: None,
            loader: None,
            loaded: Arc::new(OnceLock::new()),
        })
    }

    pub(crate) fn ensure_storage_loaded(&self) -> Result<(), String> {
        if self.loader.is_none() {
            return Ok(());
        }
        if self.loaded.get().is_none() {
            let loaded = Box::new(Self::from_archive_payload(
                self.loader
                    .as_ref()
                    .expect("stored sort column")
                    .load_sort_column()?,
            )?);
            let _ = self.loaded.set(loaded);
        }
        Ok(())
    }

    pub(crate) fn materialized(&self) -> &Self {
        self.loaded
            .get()
            .map(|loaded| loaded.as_ref())
            .unwrap_or(self)
    }

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
        records: &[RecordEntry],
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
        let mut keys = records
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
        let value_map = &index.values;
        let mut values = value_map.range(
            std::ops::Bound::Unbounded,
            std::ops::Bound::Unbounded,
            false,
        );
        while let Some((value, slots)) = values.next() {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            let key = match value {
                ScalarKey::Number(number) if index.kind == LogicalType::Number => {
                    SortKey::Number(number.clone())
                }
                ScalarKey::Bool(flag) if index.kind == LogicalType::Bool => SortKey::Bool(flag),
                ScalarKey::String(text) if index.kind == LogicalType::String => {
                    let mut bytes = Vec::new();
                    collator
                        .as_ref()
                        .expect("string collator")
                        .write_sort_key_to(&text, &mut bytes)
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
            order.extend(records.iter().map(|record| record.slot));
            order.sort_unstable_by(|a, b| {
                keys[a.0 as usize]
                    .compare_with(&keys[b.0 as usize], descending, options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        } else {
            let mut missing = Vec::new();
            let mut nulls = Vec::new();
            for record in records {
                match keys[record.slot.0 as usize] {
                    SortKey::Missing | SortKey::Unavailable => missing.push(record.slot),
                    SortKey::Null => nulls.push(record.slot),
                    _ => {}
                }
            }
            let by_id = |a: &RecordSlot, b: &RecordSlot| {
                records[a.0 as usize].id.cmp(&records[b.0 as usize].id)
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
                let mut cursor =
                    value_map.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded, true);
                while let Some((_, slots)) = cursor.next() {
                    append_inferred_group(&mut order, slots, records);
                }
            } else {
                value_map.for_each(|_, slots| {
                    append_inferred_group(&mut order, slots, records);
                    Ok(())
                })?;
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
            stored: None,
            loader: None,
            loaded: Arc::new(OnceLock::new()),
        }))
    }
}

fn append_inferred_group(
    order: &mut Vec<RecordSlot>,
    slots: &RoaringBitmap,
    records: &[RecordEntry],
) {
    let mut group = slots.iter().map(RecordSlot).collect::<Vec<_>>();
    group.sort_unstable_by(|a, b| records[a.0 as usize].id.cmp(&records[b.0 as usize].id));
    order.extend(group);
}

/// All sort columns available for a collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SortCollection {
    #[serde(with = "crate::storage::serde_support::hash_map_sequence")]
    pub columns: HashMap<FieldPath, SortColumn>,
    pub composites: Vec<CompositeSortOrder>,
    #[serde(default)]
    pub(crate) payload: Option<StoredSortPayloadRef>,
    #[serde(skip)]
    pub(crate) storage: Option<Arc<dyn SortCollectionLoader>>,
    #[serde(skip)]
    pub(crate) loaded: Arc<OnceLock<Box<SortCollection>>>,
    #[serde(skip)]
    pub(crate) load_lock: Arc<std::sync::Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredSortPayloadRef {
    pub digest: [u8; 32],
    pub length: u64,
}

pub(crate) trait SortCollectionLoader: std::fmt::Debug + Send + Sync {
    fn load_sort_collection(&self) -> Result<SortCollection, String>;
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
enum StoredSortKey {
    Missing,
    Unavailable,
    Null,
    String(Vec<u8>),
    Id(String),
    Integer(i128),
    Float(f64),
    ExactDecimal {
        negative: bool,
        digits: String,
        exponent: i64,
    },
    Bool(bool),
    Date {
        year: i32,
        month: u8,
        day: u8,
    },
    Datetime {
        unix_nanos: i128,
        offset_seconds: i32,
    },
    Unixtime(i64),
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredSortColumn {
    path: String,
    kind: String,
    keys: Vec<StoredSortKey>,
    ascending: Vec<u32>,
    descending: Vec<u32>,
    locale: String,
    strength: u8,
    numeric: bool,
    nulls: u8,
    missing: u8,
    collation_version: String,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredComposite {
    fields: Vec<(String, bool)>,
    order: Vec<u32>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct SortArchivePayload {
    columns: Vec<StoredSortColumn>,
    composites: Vec<StoredComposite>,
}

/// Explicit composite order over compatible resident scalar columns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompositeSortOrder {
    pub fields: Vec<CompositeSortPart>,
    pub order: Vec<RecordSlot>,
    #[serde(default)]
    pub(crate) stored: Option<StoredSortOrderRef>,
    #[serde(skip)]
    pub(crate) loader: Option<Arc<dyn SortOrderLoader>>,
    #[serde(skip)]
    pub(crate) loaded: Arc<OnceLock<Box<CompositeSortOrder>>>,
}

impl CompositeSortOrder {
    pub(crate) fn archive_payload(&self) -> StoredComposite {
        StoredComposite {
            fields: self
                .fields
                .iter()
                .map(|part| (part.path.display(), part.descending))
                .collect(),
            order: self.order.iter().map(|slot| slot.0).collect(),
        }
    }

    pub(crate) fn from_archive_payload(payload: StoredComposite) -> Result<Self, String> {
        Ok(Self {
            fields: payload
                .fields
                .into_iter()
                .map(|(path, descending)| {
                    Ok(CompositeSortPart {
                        path: FieldPath::parse(&path)?,
                        descending,
                    })
                })
                .collect::<Result<_, String>>()?,
            order: payload.order.into_iter().map(RecordSlot).collect(),
            stored: None,
            loader: None,
            loaded: Arc::new(OnceLock::new()),
        })
    }

    fn ensure_storage_loaded(&self) -> Result<(), String> {
        if self.loader.is_none() {
            return Ok(());
        }
        if self.loaded.get().is_none() {
            let loaded = Box::new(Self::from_archive_payload(
                self.loader
                    .as_ref()
                    .expect("stored sort order")
                    .load_sort_order()?,
            )?);
            let _ = self.loaded.set(loaded);
        }
        Ok(())
    }

    pub(crate) fn materialized(&self) -> &Self {
        self.loaded
            .get()
            .map(|loaded| loaded.as_ref())
            .unwrap_or(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredSortOrderRef {
    pub digest: [u8; 32],
    pub length: u64,
}

pub(crate) trait SortOrderLoader: std::fmt::Debug + Send + Sync {
    fn load_sort_order(&self) -> Result<StoredComposite, String>;
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

fn encode_logical_type(kind: &LogicalType) -> String {
    match kind {
        LogicalType::Unknown => "unknown".into(),
        LogicalType::String => "string".into(),
        LogicalType::Id => "id".into(),
        LogicalType::Number => "number".into(),
        LogicalType::Bool => "bool".into(),
        LogicalType::Date => "date".into(),
        LogicalType::Datetime => "datetime".into(),
        LogicalType::Unixtime => "unixtime".into(),
        LogicalType::Object => "object".into(),
        LogicalType::Array(inner) => format!("array:{}", encode_logical_type(inner)),
    }
}

fn decode_logical_type(encoded: &str) -> Result<LogicalType, String> {
    Ok(match encoded {
        "unknown" => LogicalType::Unknown,
        "string" => LogicalType::String,
        "id" => LogicalType::Id,
        "number" => LogicalType::Number,
        "bool" => LogicalType::Bool,
        "date" => LogicalType::Date,
        "datetime" => LogicalType::Datetime,
        "unixtime" => LogicalType::Unixtime,
        "object" => LogicalType::Object,
        array if array.starts_with("array:") => {
            LogicalType::Array(Box::new(decode_logical_type(&array[6..])?))
        }
        other => return Err(format!("unknown stored sort logical type {other}")),
    })
}

fn month_code(month: time::Month) -> u8 {
    match month {
        time::Month::January => 1,
        time::Month::February => 2,
        time::Month::March => 3,
        time::Month::April => 4,
        time::Month::May => 5,
        time::Month::June => 6,
        time::Month::July => 7,
        time::Month::August => 8,
        time::Month::September => 9,
        time::Month::October => 10,
        time::Month::November => 11,
        time::Month::December => 12,
    }
}

fn month_from_code(month: u8) -> Result<time::Month, String> {
    Ok(match month {
        1 => time::Month::January,
        2 => time::Month::February,
        3 => time::Month::March,
        4 => time::Month::April,
        5 => time::Month::May,
        6 => time::Month::June,
        7 => time::Month::July,
        8 => time::Month::August,
        9 => time::Month::September,
        10 => time::Month::October,
        11 => time::Month::November,
        12 => time::Month::December,
        _ => return Err(format!("invalid stored month {month}")),
    })
}

fn encode_sort_key(key: &SortKey) -> StoredSortKey {
    match key {
        SortKey::Missing => StoredSortKey::Missing,
        SortKey::Unavailable => StoredSortKey::Unavailable,
        SortKey::Null => StoredSortKey::Null,
        SortKey::String(value) => StoredSortKey::String(value.clone()),
        SortKey::Id(value) => StoredSortKey::Id(value.clone()),
        SortKey::Number(Number::Integer(value)) => StoredSortKey::Integer(*value),
        SortKey::Number(Number::Float(value)) => StoredSortKey::Float(*value),
        SortKey::Number(Number::ExactDecimal(value)) => StoredSortKey::ExactDecimal {
            negative: value.negative,
            digits: value.digits.clone(),
            exponent: value.exponent,
        },
        SortKey::Bool(value) => StoredSortKey::Bool(*value),
        SortKey::Date(value) => StoredSortKey::Date {
            year: value.year(),
            month: month_code(value.month()),
            day: value.day(),
        },
        SortKey::Datetime(value) => StoredSortKey::Datetime {
            unix_nanos: value.unix_timestamp_nanos(),
            offset_seconds: value.offset().whole_seconds(),
        },
        SortKey::Unixtime(value) => StoredSortKey::Unixtime(*value),
    }
}

fn decode_sort_key(key: StoredSortKey) -> Result<SortKey, String> {
    Ok(match key {
        StoredSortKey::Missing => SortKey::Missing,
        StoredSortKey::Unavailable => SortKey::Unavailable,
        StoredSortKey::Null => SortKey::Null,
        StoredSortKey::String(value) => SortKey::String(value),
        StoredSortKey::Id(value) => SortKey::Id(value),
        StoredSortKey::Integer(value) => SortKey::Number(Number::Integer(value)),
        StoredSortKey::Float(value) => SortKey::Number(Number::float(value)?),
        StoredSortKey::ExactDecimal {
            negative,
            digits,
            exponent,
        } => SortKey::Number(Number::ExactDecimal(
            crate::decimal::ExactDecimal::from_stored_parts(negative, digits, exponent)?,
        )),
        StoredSortKey::Bool(value) => SortKey::Bool(value),
        StoredSortKey::Date { year, month, day } => SortKey::Date(
            Date::from_calendar_date(year, month_from_code(month)?, day)
                .map_err(|error| error.to_string())?,
        ),
        StoredSortKey::Datetime {
            unix_nanos,
            offset_seconds,
        } => {
            let instant = OffsetDateTime::from_unix_timestamp_nanos(unix_nanos)
                .map_err(|error| error.to_string())?;
            let offset = time::UtcOffset::from_whole_seconds(offset_seconds)
                .map_err(|error| error.to_string())?;
            SortKey::Datetime(instant.to_offset(offset))
        }
        StoredSortKey::Unixtime(value) => SortKey::Unixtime(value),
    })
}

fn encode_strength(strength: CollationStrength) -> u8 {
    match strength {
        CollationStrength::LocaleDefault => 0,
        CollationStrength::Primary => 1,
        CollationStrength::Secondary => 2,
        CollationStrength::Tertiary => 3,
        CollationStrength::Quaternary => 4,
        CollationStrength::Identical => 5,
    }
}

fn decode_strength(value: u8) -> Result<CollationStrength, String> {
    Ok(match value {
        0 => CollationStrength::LocaleDefault,
        1 => CollationStrength::Primary,
        2 => CollationStrength::Secondary,
        3 => CollationStrength::Tertiary,
        4 => CollationStrength::Quaternary,
        5 => CollationStrength::Identical,
        _ => return Err(format!("invalid stored collation strength {value}")),
    })
}

fn encode_placement(value: Placement) -> u8 {
    match value {
        Placement::First => 0,
        Placement::Last => 1,
    }
}

fn decode_placement(value: u8) -> Result<Placement, String> {
    Ok(match value {
        0 => Placement::First,
        1 => Placement::Last,
        _ => return Err(format!("invalid stored placement {value}")),
    })
}

impl SortCollection {
    #[allow(dead_code)]
    pub(crate) fn archive_payload(&self) -> Result<SortArchivePayload, String> {
        let mut columns = self
            .columns
            .values()
            .map(|column| StoredSortColumn {
                path: column.path.display(),
                kind: encode_logical_type(&column.kind),
                keys: column.keys.iter().map(encode_sort_key).collect(),
                ascending: column.ascending.iter().map(|slot| slot.0).collect(),
                descending: column.descending.iter().map(|slot| slot.0).collect(),
                locale: column.locale.clone(),
                strength: encode_strength(column.options.strength),
                numeric: column.options.numeric,
                nulls: encode_placement(column.options.nulls),
                missing: encode_placement(column.options.missing),
                collation_version: column.collation_version.clone(),
            })
            .collect::<Vec<_>>();
        columns.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(SortArchivePayload {
            columns,
            composites: self
                .composites
                .iter()
                .map(|composite| StoredComposite {
                    fields: composite
                        .fields
                        .iter()
                        .map(|part| (part.path.display(), part.descending))
                        .collect(),
                    order: composite.order.iter().map(|slot| slot.0).collect(),
                })
                .collect(),
        })
    }

    pub(crate) fn from_archive_payload(payload: SortArchivePayload) -> Result<Self, String> {
        let mut columns = HashMap::with_capacity(payload.columns.len());
        for column in payload.columns {
            let path = FieldPath::parse(&column.path)?;
            columns.insert(
                path.clone(),
                SortColumn {
                    path,
                    kind: decode_logical_type(&column.kind)?,
                    keys: column
                        .keys
                        .into_iter()
                        .map(decode_sort_key)
                        .collect::<Result<_, String>>()?,
                    ascending: column.ascending.into_iter().map(RecordSlot).collect(),
                    descending: column.descending.into_iter().map(RecordSlot).collect(),
                    locale: column.locale,
                    options: SortOptions {
                        strength: decode_strength(column.strength)?,
                        numeric: column.numeric,
                        nulls: decode_placement(column.nulls)?,
                        missing: decode_placement(column.missing)?,
                    },
                    collation_version: column.collation_version,
                    stored: None,
                    loader: None,
                    loaded: Arc::new(OnceLock::new()),
                },
            );
        }
        Ok(Self {
            columns,
            composites: payload
                .composites
                .into_iter()
                .map(|composite| {
                    Ok(CompositeSortOrder {
                        fields: composite
                            .fields
                            .into_iter()
                            .map(|(path, descending)| {
                                Ok(CompositeSortPart {
                                    path: FieldPath::parse(&path)?,
                                    descending,
                                })
                            })
                            .collect::<Result<_, String>>()?,
                        order: composite.order.into_iter().map(RecordSlot).collect(),
                        stored: None,
                        loader: None,
                        loaded: Arc::new(OnceLock::new()),
                    })
                })
                .collect::<Result<_, String>>()?,
            payload: None,
            storage: None,
            loaded: Arc::new(OnceLock::new()),
            load_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    pub(crate) fn ensure_storage_loaded(&self) -> Result<(), String> {
        if self.storage.is_none() {
            return Ok(());
        }
        let _guard = self
            .load_lock
            .lock()
            .map_err(|_| "sort collection load lock poisoned".to_string())?;
        if self.loaded.get().is_none() {
            let mut loaded = self
                .storage
                .as_ref()
                .expect("stored sort collection")
                .load_sort_collection()?;
            loaded.payload = None;
            loaded.storage = None;
            loaded.loaded = Arc::new(OnceLock::new());
            loaded.load_lock = Arc::new(std::sync::Mutex::new(()));
            self.loaded
                .set(Box::new(loaded))
                .map_err(|_| "sort collection was initialized concurrently".to_string())?;
        }
        Ok(())
    }

    pub(crate) fn ensure_all_loaded(&self) -> Result<(), String> {
        for column in self.columns.values() {
            column.ensure_storage_loaded()?;
        }
        for composite in &self.composites {
            composite.ensure_storage_loaded()?;
        }
        Ok(())
    }

    pub(crate) fn may_resolve_requested_order(
        &self,
        fields: &[crate::execution_options::SortField],
    ) -> bool {
        let this = self.materialized();
        if fields.is_empty() {
            return true;
        }
        if fields.len() == 1 {
            return FieldPath::parse(&fields[0].field)
                .is_ok_and(|path| this.columns.contains_key(&path));
        }
        this.composites.iter().any(|composite| {
            composite.fields.len() == fields.len()
                && composite
                    .fields
                    .iter()
                    .zip(fields)
                    .all(|(stored, requested)| {
                        stored.path.display() == requested.field
                            && stored.descending == requested.descending
                    })
        })
    }

    pub(crate) fn materialized(&self) -> &Self {
        self.loaded
            .get()
            .map(|loaded| loaded.as_ref())
            .unwrap_or(self)
    }

    pub(crate) fn configuration_only(
        paths: &[FieldPath],
        schema: &Schema,
        locale: &str,
        defaults: SortOptions,
        overrides: &HashMap<FieldPath, SortConfig>,
    ) -> Result<Self, String> {
        if overrides.keys().any(|path| !paths.contains(path)) {
            return Err("sort override is not a configured sort field".into());
        }
        let mut columns = HashMap::with_capacity(paths.len());
        for path in paths {
            let config = overrides.get(path).cloned().unwrap_or_else(|| SortConfig {
                locale: locale.into(),
                options: defaults,
            });
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
                        stored: None,
                        loader: None,
                        loaded: Arc::new(OnceLock::new()),
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
            composites.push(CompositeSortOrder {
                fields: parts.expect("validated composite parts"),
                order: Vec::new(),
                stored: None,
                loader: None,
                loaded: Arc::new(OnceLock::new()),
            });
        }
        Ok(Self {
            columns,
            composites,
            payload: None,
            storage: None,
            loaded: Arc::new(OnceLock::new()),
            load_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    pub fn composite(&self, fields: &[SortField]) -> Option<&CompositeSortOrder> {
        let normalized = fields
            .iter()
            .map(simple_composite_part)
            .collect::<Option<Vec<_>>>()?;
        let composite = self
            .materialized()
            .composites
            .iter()
            .find(|entry| entry.fields == normalized)?;
        composite.ensure_storage_loaded().ok()?;
        Some(composite.materialized())
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
    pub(crate) fn with_keys(
        paths: &[FieldPath],
        schema: &Schema,
        locale: &str,
        defaults: SortOptions,
        overrides: &HashMap<FieldPath, SortConfig>,
        mut keys: HashMap<FieldPath, Vec<SortKey>>,
        record_count: usize,
    ) -> Result<Self, String> {
        let mut builder = Self::new(paths, schema, locale, defaults, overrides)?;
        if keys.len() != builder.columns.len() {
            return Err("sort batch column coverage mismatch".into());
        }
        for (path, column) in &mut builder.columns {
            let values = keys.remove(path).ok_or("sort batch column missing")?;
            if values.len() != record_count {
                return Err("sort batch key coverage mismatch".into());
            }
            column.keys = values;
        }
        if !keys.is_empty() {
            return Err("sort batch contains an unexpected column".into());
        }
        Ok(builder)
    }

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
                        stored: None,
                        loader: None,
                        loaded: Arc::new(OnceLock::new()),
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
                CompositeSortOrder {
                    fields,
                    order,
                    stored: None,
                    loader: None,
                    loaded: Arc::new(OnceLock::new()),
                }
            })
            .collect();
        Ok(SortCollection {
            columns: self.columns,
            composites,
            payload: None,
            storage: None,
            loaded: Arc::new(OnceLock::new()),
            load_lock: Arc::new(std::sync::Mutex::new(())),
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
        self.refresh_file_status_for_records(&generation.records, |file| {
            inventory.files[file.0 as usize].status.code()
        });
    }

    /// Finalize status-dependent orders from a segment's local file table.
    pub(crate) fn refresh_file_status_for_records(
        &mut self,
        records: &[RecordEntry],
        status: impl Fn(crate::model::FileId) -> u8,
    ) {
        let mut refreshed = false;
        for column in self.columns.values_mut() {
            if column.path.root != PathRoot::Metadata(MetadataField::FileStatus) {
                continue;
            }
            refreshed = true;
            for record in records {
                column.keys[record.slot.0 as usize] =
                    SortKey::Number(Number::integer(status(record.file) as i128));
            }
            column.ascending = records.iter().map(|r| r.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
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
                    compare_composite(&columns, *a, *b)
                        .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
                });
            }
        }
    }

    pub fn column(&self, field: &str) -> Result<&SortColumn, String> {
        let path = FieldPath::parse(field)?;
        let column = self
            .materialized()
            .columns
            .get(&path)
            .ok_or_else(|| format!("sort field is not registered: {field}"))?;
        column.ensure_storage_loaded()?;
        Ok(column.materialized())
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
