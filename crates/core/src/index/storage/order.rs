//! Sort configuration and comparison of logical order keys.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::{FieldPath, Number};
use crate::query::options::SortField;

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
