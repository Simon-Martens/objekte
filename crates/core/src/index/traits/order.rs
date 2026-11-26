//! Sort configuration and comparison of logical order keys.

pub(crate) mod declared;
pub(crate) mod inferred_cache;
pub(crate) mod key_transfer;

use std::borrow::Cow;
use std::cmp::Ordering;
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

use crate::model::{FieldPath, Number};
use crate::query::options::SortField;

use crate::model::{LogicalType, RecordSlot, Value};
use crate::query::Operator;
use crate::query_access::membership::MembershipView;

use super::ordered_slots::OrderedSlots;
use super::transfer::{BatchLimit, BatchStatus, StorageResult};

/// Keys and traversal for one pinned scalar sort specification.
pub(crate) trait OrderColumnReader: Send + Sync {
    fn path(&self) -> &FieldPath;
    fn kind(&self) -> &LogicalType;
    fn collation_version(&self) -> &str;
    fn options(&self) -> SortOptions;
    fn locale(&self) -> &str;
    fn retained_bytes(&self) -> usize;
    fn key_count(&self) -> usize;

    /// Filters a query-owned input domain. A reader can decline an operation
    /// it cannot prove. An error from an attempted read is propagated.
    fn match_scalar(
        &self,
        _predicate: &ScalarOrderPredicate,
        _input: MembershipView<'_>,
        _ordered_only: bool,
    ) -> StorageResult<Option<RoaringBitmap>> {
        Ok(None)
    }

    fn key(&self, slot: RecordSlot) -> StorageResult<Cow<'_, SortKey>>;
    fn key_batch(
        &self,
        slots: &[RecordSlot],
        limit: BatchLimit,
    ) -> StorageResult<(Vec<Cow<'_, SortKey>>, BatchStatus)> {
        let keys = key_transfer::read_order_keys(self, slots, limit)?;
        let status = if keys.len() == slots.len() {
            BatchStatus::Done
        } else {
            BatchStatus::More
        };
        Ok((keys, status))
    }

    fn slots(&self, descending: bool) -> StorageResult<&dyn OrderedSlots>;
}

pub(crate) struct ScalarOrderPredicate {
    pub literal: SortKey,
    pub operator: Operator,
}

impl ScalarOrderPredicate {
    pub(crate) fn new(value: &Value, operator: Operator) -> Option<Self> {
        let literal = match value {
            Value::Number(value) => SortKey::Number(value.clone()),
            Value::Date(value) => SortKey::Date(*value),
            Value::Datetime(value) => SortKey::Datetime(*value),
            Value::Unixtime(value) => SortKey::Unixtime(*value),
            _ => return None,
        };
        Some(Self { literal, operator })
    }

    pub(crate) fn matches(&self, key: &SortKey) -> bool {
        if matches!(key, SortKey::Missing | SortKey::Null) {
            return false;
        }
        match self.operator {
            Operator::Eq => key.compare(&self.literal, false).is_eq(),
            Operator::Ne => key.compare(&self.literal, false).is_ne(),
            Operator::Gt => key.compare(&self.literal, false).is_gt(),
            Operator::Ge => key.compare(&self.literal, false).is_ge(),
            Operator::Lt => key.compare(&self.literal, false).is_lt(),
            Operator::Le => key.compare(&self.literal, false).is_le(),
            _ => false,
        }
    }
}

/// A query-created scalar order requested against one pinned segment.
pub(crate) struct PostingOrderRequest<'a> {
    pub path: &'a FieldPath,
    pub locale: &'a str,
    pub options: SortOptions,
    pub descending: bool,
    pub execution: Option<&'a crate::query_execution::streaming::ExecutionContext>,
}

/// Owned logical keys and an ordered slot sequence prepared from a complete
/// scalar dictionary. The destination can adopt or encode these buffers.
pub(crate) struct InferredOrderRows {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub config: SortConfig,
    pub keys: Vec<SortKey>,
    pub order: Vec<RecordSlot>,
    pub descending: bool,
}

/// Metadata selected before any key contributions arrive.
#[derive(Clone)]
pub(crate) struct DeclaredOrderDefinition {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub config: SortConfig,
    pub collation_version: String,
}

/// Record IDs resolve stable ties using the segment's established slots.
pub(crate) trait OrderIdentities {
    fn len(&self) -> usize;
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "consumed by the independent conformance writer")
    )]
    fn id(&self, slot: RecordSlot) -> &crate::model::RecordId;
}

impl<R: crate::records::RecordIdSlot> OrderIdentities for &[R] {
    fn len(&self) -> usize {
        <[R]>::len(self)
    }

    fn id(&self, slot: RecordSlot) -> &crate::model::RecordId {
        self[slot.0 as usize].record_id()
    }
}

pub(crate) trait DeclaredOrderWriter: Send {
    fn push_keys(&mut self, base: RecordSlot, keys: &[SortKey]) -> StorageResult<()>;
    fn seal(
        self: Box<Self>,
        identities: &dyn OrderIdentities,
    ) -> StorageResult<Arc<dyn OrderColumnReader>>;
}

pub(crate) trait DeclaredOrderStore: Send + Sync {
    fn accepts(&self, path: &FieldPath, kind: &LogicalType) -> bool;
    fn start(
        &self,
        definition: DeclaredOrderDefinition,
    ) -> StorageResult<Box<dyn DeclaredOrderWriter>>;
}

/// Inferred columns receive assigned keys in dictionary order. Unassigned
/// slots are missing. Writers retain the requested direction and break ties
/// using the supplied segment identities.
pub(crate) trait InferredOrderWriter: Send {
    fn assign(&mut self, key: &SortKey, slots: &[RecordSlot]) -> StorageResult<()>;
    fn seal(
        self: Box<Self>,
        identities: &dyn OrderIdentities,
    ) -> StorageResult<Arc<dyn OrderColumnReader>>;
}

pub(crate) trait InferredOrderStore: Send + Sync {
    fn start(
        &self,
        definition: DeclaredOrderDefinition,
        descending: bool,
    ) -> StorageResult<Box<dyn InferredOrderWriter>>;
}

pub(crate) const ORDER_KEY_BATCH_SIZE: usize = 128;

pub const COLLATION_VERSION: &str =
    "icu_collator-2.4.0/icu_collator_data-2.4.0/CLDR-49.0.0/Unicode-18.0.0";

/// Fixed binary identity for one resident composite constituent.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub(crate) fn rank(&self, options: SortOptions) -> i8 {
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
            // TODO: how to compare differing types
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
