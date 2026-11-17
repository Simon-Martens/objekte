//! Typed B-tree dictionaries with record-slot postings and special states.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{Arc, OnceLock};

use rkyv::collections::btree_map::{ArchivedBTreeMap, BTreeMapResolver};
use rkyv::place::Place;
use rkyv::vec::{ArchivedVec, VecResolver};
use rkyv::{Archive, Archived, Deserialize as ArchiveDeserialize, Serialize as ArchiveSerialize};
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::archive_storage::ValidatedArchive;
use crate::field_state::{StatePredicate, value_matches};
use crate::inventory::append_shifted_bitmap;
use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::query::{Operator, Quantifier};
use crate::reference::{BoundTerm, Derived, Transform};
use crate::reference::{FieldDefinition, Schema, data_values_small};

pub(crate) trait PostingIndexLoader: fmt::Debug + Send + Sync {
    fn load_posting_index(&self) -> Result<PostingIndexSnapshot, String>;

    fn stored_payload_reference(&self) -> Option<StoredPostingPayloadRef> {
        None
    }
}

pub(crate) trait PostingLeafLoader: fmt::Debug + Send + Sync {
    fn load_posting_leaf(&self, reference: &StoredPostingLeafRef) -> Result<RoaringBitmap, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredPostingLeafRef {
    pub offset: u64,
    pub length: u64,
    pub cardinality: u64,
    pub decoded_bytes_estimate: u64,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum ScalarKey {
    String(String),
    Id(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
}

/// Native key stored in completed posting archives.
///
/// The stored variants retain the original numeric representation while the
/// comparison implementation preserves `Number`'s cross-representation order.
#[derive(Clone, Debug, Archive, ArchiveSerialize, ArchiveDeserialize, Serialize, Deserialize)]
pub(crate) enum PostingNumberKey {
    Integer(i128),
    Float(f64),
    ExactDecimal {
        negative: bool,
        digits: String,
        exponent: i64,
    },
}

/// Native key stored in completed scalar posting archives.
#[derive(Clone, Debug, Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) enum StoredScalarKey {
    String(String),
    Id(String),
    Number(PostingNumberKey),
    Bool(bool),
    Date(i32),
    Datetime {
        unix_nanos: i128,
        offset_seconds: i32,
    },
    Unixtime(i64),
}

#[derive(Clone, Copy)]
enum BorrowedNumber<'a> {
    Integer(i128),
    Float(f64),
    Exact {
        negative: bool,
        digits: &'a str,
        exponent: i64,
    },
}

impl<'a> From<&'a PostingNumberKey> for BorrowedNumber<'a> {
    fn from(number: &'a PostingNumberKey) -> Self {
        match number {
            PostingNumberKey::Integer(value) => Self::Integer(*value),
            PostingNumberKey::Float(value) => Self::Float(*value),
            PostingNumberKey::ExactDecimal {
                negative,
                digits,
                exponent,
            } => Self::Exact {
                negative: *negative,
                digits,
                exponent: *exponent,
            },
        }
    }
}

impl<'a> From<&'a Number> for BorrowedNumber<'a> {
    fn from(number: &'a Number) -> Self {
        match number {
            Number::Integer(value) => Self::Integer(*value),
            Number::Float(value) => Self::Float(*value),
            Number::ExactDecimal(decimal) => Self::Exact {
                negative: decimal.negative,
                digits: &decimal.digits,
                exponent: decimal.exponent,
            },
        }
    }
}

impl<'a> From<&'a ArchivedPostingNumberKey> for BorrowedNumber<'a> {
    fn from(number: &'a ArchivedPostingNumberKey) -> Self {
        match number {
            ArchivedPostingNumberKey::Integer(value) => Self::Integer(value.to_native()),
            ArchivedPostingNumberKey::Float(value) => Self::Float(value.to_native()),
            ArchivedPostingNumberKey::ExactDecimal {
                negative,
                digits,
                exponent,
            } => Self::Exact {
                negative: bool::from(*negative),
                digits: digits.as_str(),
                exponent: exponent.to_native(),
            },
        }
    }
}

fn compare_int_float(integer: i128, float: f64) -> Ordering {
    if !float.is_finite() {
        return 0.0f64.total_cmp(&float);
    }
    if float >= 2f64.powi(127) {
        return Ordering::Less;
    }
    if float < i128::MIN as f64 {
        return Ordering::Greater;
    }
    let truncated = float.trunc() as i128;
    match integer.cmp(&truncated) {
        Ordering::Equal if float.fract() > 0.0 => Ordering::Less,
        Ordering::Equal if float.fract() < 0.0 => Ordering::Greater,
        result => result,
    }
}

fn compare_exact(
    left_negative: bool,
    left_digits: &str,
    left_exponent: i64,
    right_negative: bool,
    right_digits: &str,
    right_exponent: i64,
) -> Ordering {
    if left_digits == "0" {
        return if right_digits == "0" {
            Ordering::Equal
        } else if right_negative {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }
    if right_digits == "0" {
        return if left_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if left_negative != right_negative {
        return if left_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let sign = if left_negative {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    let left_magnitude = left_digits.len() as i128 + left_exponent as i128;
    let right_magnitude = right_digits.len() as i128 + right_exponent as i128;
    let compared = left_magnitude.cmp(&right_magnitude);
    if compared != Ordering::Equal {
        return if sign == Ordering::Greater {
            compared
        } else {
            compared.reverse()
        };
    }
    let left = left_digits.as_bytes();
    let right = right_digits.as_bytes();
    let compared = (0..left.len().max(right.len()))
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or(b'0')
                .cmp(&right.get(index).copied().unwrap_or(b'0'))
        })
        .find(|order| *order != Ordering::Equal)
        .unwrap_or(Ordering::Equal);
    if sign == Ordering::Greater {
        compared
    } else {
        compared.reverse()
    }
}

fn compare_exact_integer(negative: bool, digits: &str, exponent: i64, integer: i128) -> Ordering {
    let integer_text = integer.to_string();
    let (integer_negative, integer_digits) = integer_text
        .strip_prefix('-')
        .map_or((false, integer_text.as_str()), |digits| (true, digits));
    compare_exact(
        negative,
        digits,
        exponent,
        integer_negative,
        integer_digits,
        0,
    )
}

fn compare_exact_float(negative: bool, digits: &str, exponent: i64, float: f64) -> Ordering {
    if !float.is_finite() {
        return if float.is_sign_negative() {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }
    let exact = crate::decimal::ExactDecimal::from_f64(float);
    compare_exact(
        negative,
        digits,
        exponent,
        exact.negative,
        &exact.digits,
        exact.exponent,
    )
}

fn compare_borrowed_numbers(left: BorrowedNumber<'_>, right: BorrowedNumber<'_>) -> Ordering {
    use BorrowedNumber::{Exact, Float, Integer};
    match (left, right) {
        (Integer(left), Integer(right)) => left.cmp(&right),
        (Float(left), Float(right)) if left == 0.0 && right == 0.0 => Ordering::Equal,
        (Float(left), Float(right)) => left.total_cmp(&right),
        (Integer(left), Float(right)) => compare_int_float(left, right),
        (Float(left), Integer(right)) => compare_int_float(right, left).reverse(),
        (
            Exact {
                negative: left_negative,
                digits: left_digits,
                exponent: left_exponent,
            },
            Exact {
                negative: right_negative,
                digits: right_digits,
                exponent: right_exponent,
            },
        ) => compare_exact(
            left_negative,
            left_digits,
            left_exponent,
            right_negative,
            right_digits,
            right_exponent,
        ),
        (
            Exact {
                negative,
                digits,
                exponent,
            },
            Integer(right),
        ) => compare_exact_integer(negative, digits, exponent, right),
        (
            Integer(left),
            Exact {
                negative,
                digits,
                exponent,
            },
        ) => compare_exact_integer(negative, digits, exponent, left).reverse(),
        (Exact { .. }, Float(right)) if !right.is_finite() => 0.0f64.total_cmp(&right),
        (Float(left), Exact { .. }) if !left.is_finite() => 0.0f64.total_cmp(&left).reverse(),
        (
            Exact {
                negative,
                digits,
                exponent,
            },
            Float(right),
        ) => compare_exact_float(negative, digits, exponent, right),
        (
            Float(left),
            Exact {
                negative,
                digits,
                exponent,
            },
        ) => compare_exact_float(negative, digits, exponent, left).reverse(),
    }
}

impl PartialEq for PostingNumberKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PostingNumberKey {}

impl PartialOrd for PostingNumberKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PostingNumberKey {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_borrowed_numbers(self.into(), other.into())
    }
}

impl PartialEq for ArchivedPostingNumberKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ArchivedPostingNumberKey {}

impl PartialOrd for ArchivedPostingNumberKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ArchivedPostingNumberKey {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_borrowed_numbers(self.into(), other.into())
    }
}

fn stored_scalar_variant(key: &StoredScalarKey) -> u8 {
    match key {
        StoredScalarKey::String(_) => 0,
        StoredScalarKey::Id(_) => 1,
        StoredScalarKey::Number(_) => 2,
        StoredScalarKey::Bool(_) => 3,
        StoredScalarKey::Date(_) => 4,
        StoredScalarKey::Datetime { .. } => 5,
        StoredScalarKey::Unixtime(_) => 6,
    }
}

fn archived_scalar_variant(key: &rkyv::Archived<StoredScalarKey>) -> u8 {
    match key {
        rkyv::Archived::<StoredScalarKey>::String(_) => 0,
        rkyv::Archived::<StoredScalarKey>::Id(_) => 1,
        rkyv::Archived::<StoredScalarKey>::Number(_) => 2,
        rkyv::Archived::<StoredScalarKey>::Bool(_) => 3,
        rkyv::Archived::<StoredScalarKey>::Date(_) => 4,
        rkyv::Archived::<StoredScalarKey>::Datetime { .. } => 5,
        rkyv::Archived::<StoredScalarKey>::Unixtime(_) => 6,
    }
}

impl PartialEq for StoredScalarKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for StoredScalarKey {}

impl PartialOrd for StoredScalarKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StoredScalarKey {
    fn cmp(&self, other: &Self) -> Ordering {
        stored_scalar_variant(self)
            .cmp(&stored_scalar_variant(other))
            .then_with(|| match (self, other) {
                (StoredScalarKey::String(left), StoredScalarKey::String(right))
                | (StoredScalarKey::Id(left), StoredScalarKey::Id(right)) => left.cmp(right),
                (StoredScalarKey::Number(left), StoredScalarKey::Number(right)) => left.cmp(right),
                (StoredScalarKey::Bool(left), StoredScalarKey::Bool(right)) => left.cmp(right),
                (StoredScalarKey::Date(left), StoredScalarKey::Date(right)) => left.cmp(right),
                (
                    StoredScalarKey::Datetime {
                        unix_nanos: left, ..
                    },
                    StoredScalarKey::Datetime {
                        unix_nanos: right, ..
                    },
                ) => left.cmp(right),
                (StoredScalarKey::Unixtime(left), StoredScalarKey::Unixtime(right)) => {
                    left.cmp(right)
                }
                _ => Ordering::Equal,
            })
    }
}

impl PartialEq for ArchivedStoredScalarKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ArchivedStoredScalarKey {}

impl PartialOrd for ArchivedStoredScalarKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ArchivedStoredScalarKey {
    fn cmp(&self, other: &Self) -> Ordering {
        archived_scalar_variant(self)
            .cmp(&archived_scalar_variant(other))
            .then_with(|| match (self, other) {
                (
                    rkyv::Archived::<StoredScalarKey>::String(left),
                    rkyv::Archived::<StoredScalarKey>::String(right),
                )
                | (
                    rkyv::Archived::<StoredScalarKey>::Id(left),
                    rkyv::Archived::<StoredScalarKey>::Id(right),
                ) => left.as_str().cmp(right.as_str()),
                (
                    rkyv::Archived::<StoredScalarKey>::Number(left),
                    rkyv::Archived::<StoredScalarKey>::Number(right),
                ) => left.cmp(right),
                (
                    rkyv::Archived::<StoredScalarKey>::Bool(left),
                    rkyv::Archived::<StoredScalarKey>::Bool(right),
                ) => bool::from(*left).cmp(&bool::from(*right)),
                (
                    rkyv::Archived::<StoredScalarKey>::Date(left),
                    rkyv::Archived::<StoredScalarKey>::Date(right),
                ) => left.to_native().cmp(&right.to_native()),
                (
                    rkyv::Archived::<StoredScalarKey>::Datetime {
                        unix_nanos: left, ..
                    },
                    rkyv::Archived::<StoredScalarKey>::Datetime {
                        unix_nanos: right, ..
                    },
                ) => left.to_native().cmp(&right.to_native()),
                (
                    rkyv::Archived::<StoredScalarKey>::Unixtime(left),
                    rkyv::Archived::<StoredScalarKey>::Unixtime(right),
                ) => left.to_native().cmp(&right.to_native()),
                _ => Ordering::Equal,
            })
    }
}

impl PostingNumberKey {
    pub(crate) fn from_number(number: &Number) -> Self {
        match number {
            Number::Integer(value) => Self::Integer(*value),
            Number::Float(value) => Self::Float(*value),
            Number::ExactDecimal(decimal) => Self::ExactDecimal {
                negative: decimal.negative,
                digits: decimal.digits.clone(),
                exponent: decimal.exponent,
            },
        }
    }

    pub(crate) fn from_number_owned(number: Number) -> Self {
        match number {
            Number::Integer(value) => Self::Integer(value),
            Number::Float(value) => Self::Float(value),
            Number::ExactDecimal(decimal) => Self::ExactDecimal {
                negative: decimal.negative,
                digits: decimal.digits,
                exponent: decimal.exponent,
            },
        }
    }

    pub(crate) fn to_number(&self) -> Number {
        match self {
            Self::Integer(value) => Number::Integer(*value),
            Self::Float(value) => Number::Float(*value),
            Self::ExactDecimal {
                negative,
                digits,
                exponent,
            } => Number::ExactDecimal(crate::decimal::ExactDecimal {
                negative: *negative,
                digits: digits.clone(),
                exponent: *exponent,
            }),
        }
    }
}

impl StoredScalarKey {
    pub(crate) fn from_scalar(key: &ScalarKey) -> Self {
        match key {
            ScalarKey::String(value) => Self::String(value.clone()),
            ScalarKey::Id(value) => Self::Id(value.clone()),
            ScalarKey::Number(value) => Self::Number(PostingNumberKey::from_number(value)),
            ScalarKey::Bool(value) => Self::Bool(*value),
            ScalarKey::Date(value) => Self::Date(value.to_julian_day()),
            ScalarKey::Datetime(value) => Self::Datetime {
                unix_nanos: value.unix_timestamp_nanos(),
                offset_seconds: value.offset().whole_seconds(),
            },
            ScalarKey::Unixtime(value) => Self::Unixtime(*value),
        }
    }

    pub(crate) fn from_scalar_owned(key: ScalarKey) -> Self {
        match key {
            ScalarKey::String(value) => Self::String(value),
            ScalarKey::Id(value) => Self::Id(value),
            ScalarKey::Number(value) => Self::Number(PostingNumberKey::from_number_owned(value)),
            ScalarKey::Bool(value) => Self::Bool(value),
            ScalarKey::Date(value) => Self::Date(value.to_julian_day()),
            ScalarKey::Datetime(value) => Self::Datetime {
                unix_nanos: value.unix_timestamp_nanos(),
                offset_seconds: value.offset().whole_seconds(),
            },
            ScalarKey::Unixtime(value) => Self::Unixtime(value),
        }
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::String(value) => Self::String(value.clone()),
            Value::Id(value) => Self::Id(value.clone()),
            Value::Number(value) => Self::Number(PostingNumberKey::from_number(value)),
            Value::Bool(value) => Self::Bool(*value),
            Value::Date(value) => Self::Date(value.to_julian_day()),
            Value::Datetime(value) => Self::Datetime {
                unix_nanos: value.unix_timestamp_nanos(),
                offset_seconds: value.offset().whole_seconds(),
            },
            Value::Unixtime(value) => Self::Unixtime(*value),
            _ => return None,
        })
    }

    pub(crate) fn to_scalar(&self) -> ScalarKey {
        match self {
            Self::String(value) => ScalarKey::String(value.clone()),
            Self::Id(value) => ScalarKey::Id(value.clone()),
            Self::Number(value) => ScalarKey::Number(value.to_number()),
            Self::Bool(value) => ScalarKey::Bool(*value),
            Self::Date(value) => {
                ScalarKey::Date(Date::from_julian_day(*value).expect("validated stored Julian day"))
            }
            Self::Datetime {
                unix_nanos,
                offset_seconds,
            } => {
                let utc = OffsetDateTime::from_unix_timestamp_nanos(*unix_nanos)
                    .expect("validated stored UTC instant");
                let offset = time::UtcOffset::from_whole_seconds(*offset_seconds)
                    .expect("validated stored UTC offset");
                ScalarKey::Datetime(utc.to_offset(offset))
            }
            Self::Unixtime(value) => ScalarKey::Unixtime(*value),
        }
    }
}

fn compare_scalar_to_archived(
    query: &ScalarKey,
    archived: &rkyv::Archived<StoredScalarKey>,
) -> Ordering {
    let query_variant = match query {
        ScalarKey::String(_) => 0u8,
        ScalarKey::Id(_) => 1,
        ScalarKey::Number(_) => 2,
        ScalarKey::Bool(_) => 3,
        ScalarKey::Date(_) => 4,
        ScalarKey::Datetime(_) => 5,
        ScalarKey::Unixtime(_) => 6,
    };
    query_variant
        .cmp(&archived_scalar_variant(archived))
        .then_with(|| match (query, archived) {
            (ScalarKey::String(query), rkyv::Archived::<StoredScalarKey>::String(archived))
            | (ScalarKey::Id(query), rkyv::Archived::<StoredScalarKey>::Id(archived)) => {
                query.as_str().cmp(archived.as_str())
            }
            (ScalarKey::Number(query), rkyv::Archived::<StoredScalarKey>::Number(archived)) => {
                compare_borrowed_numbers(query.into(), archived.into())
            }
            (ScalarKey::Bool(query), rkyv::Archived::<StoredScalarKey>::Bool(archived)) => {
                query.cmp(&bool::from(*archived))
            }
            (ScalarKey::Date(query), rkyv::Archived::<StoredScalarKey>::Date(archived)) => {
                query.to_julian_day().cmp(&archived.to_native())
            }
            (
                ScalarKey::Datetime(query),
                rkyv::Archived::<StoredScalarKey>::Datetime {
                    unix_nanos: archived,
                    ..
                },
            ) => query.unix_timestamp_nanos().cmp(&archived.to_native()),
            (ScalarKey::Unixtime(query), rkyv::Archived::<StoredScalarKey>::Unixtime(archived)) => {
                query.cmp(&archived.to_native())
            }
            _ => Ordering::Equal,
        })
}

pub(crate) trait PostingDictionaryKey: Clone + Ord + Archive + Sized
where
    Self::Archived: Ord,
{
    type Query: Ord + Sized;
    type Public: Ord + PartialEq;

    fn stored_query(query: &Self::Query) -> Self;
    fn stored_from_public(value: &Self::Public) -> Self;
    fn stored_from_public_owned(value: Self::Public) -> Self;
    fn compare_query(query: &Self::Query, archived: &Self::Archived) -> Ordering;
    fn public_from_stored(value: &Self) -> Self::Public;
    fn public_from_archived(value: &Self::Archived) -> Self::Public;
    fn empty_published() -> &'static PublishedPostingDictionary<Self>
    where
        Self: Sized;
}

impl PostingDictionaryKey for StoredScalarKey {
    type Query = ScalarKey;
    type Public = ScalarKey;

    fn stored_query(query: &Self::Query) -> Self {
        Self::from_scalar(query)
    }

    fn stored_from_public(value: &Self::Public) -> Self {
        Self::from_scalar(value)
    }

    fn stored_from_public_owned(value: Self::Public) -> Self {
        Self::from_scalar_owned(value)
    }

    fn compare_query(query: &Self::Query, archived: &Self::Archived) -> Ordering {
        compare_scalar_to_archived(query, archived)
    }

    fn public_from_stored(value: &Self) -> Self::Public {
        value.to_scalar()
    }

    fn public_from_archived(value: &Self::Archived) -> Self::Public {
        scalar_from_archived(value)
    }

    fn empty_published() -> &'static PublishedPostingDictionary<Self> {
        static EMPTY: OnceLock<PublishedPostingDictionary<StoredScalarKey>> = OnceLock::new();
        EMPTY.get_or_init(empty_scalar_published_dictionary)
    }
}

impl PostingDictionaryKey for PostingNumberKey {
    type Query = Number;
    type Public = Number;

    fn stored_query(query: &Self::Query) -> Self {
        Self::from_number(query)
    }

    fn stored_from_public(value: &Self::Public) -> Self {
        Self::from_number(value)
    }

    fn stored_from_public_owned(value: Self::Public) -> Self {
        Self::from_number_owned(value)
    }

    fn compare_query(query: &Self::Query, archived: &Self::Archived) -> Ordering {
        compare_borrowed_numbers(query.into(), archived.into())
    }

    fn public_from_stored(value: &Self) -> Self::Public {
        value.to_number()
    }

    fn public_from_archived(value: &Self::Archived) -> Self::Public {
        number_from_archived(value)
    }

    fn empty_published() -> &'static PublishedPostingDictionary<Self> {
        static EMPTY: OnceLock<PublishedPostingDictionary<PostingNumberKey>> = OnceLock::new();
        EMPTY.get_or_init(empty_number_published_dictionary)
    }
}

fn scalar_from_archived(key: &rkyv::Archived<StoredScalarKey>) -> ScalarKey {
    match key {
        rkyv::Archived::<StoredScalarKey>::String(value) => {
            ScalarKey::String(value.as_str().into())
        }
        rkyv::Archived::<StoredScalarKey>::Id(value) => ScalarKey::Id(value.as_str().into()),
        rkyv::Archived::<StoredScalarKey>::Number(value) => {
            ScalarKey::Number(number_from_archived(value))
        }
        rkyv::Archived::<StoredScalarKey>::Bool(value) => ScalarKey::Bool(bool::from(*value)),
        rkyv::Archived::<StoredScalarKey>::Date(value) => ScalarKey::Date(
            Date::from_julian_day(value.to_native()).expect("validated stored Julian day"),
        ),
        rkyv::Archived::<StoredScalarKey>::Datetime {
            unix_nanos,
            offset_seconds,
        } => {
            let utc = OffsetDateTime::from_unix_timestamp_nanos(unix_nanos.to_native())
                .expect("validated stored UTC instant");
            let offset = time::UtcOffset::from_whole_seconds(offset_seconds.to_native())
                .expect("validated stored UTC offset");
            ScalarKey::Datetime(utc.to_offset(offset))
        }
        rkyv::Archived::<StoredScalarKey>::Unixtime(value) => {
            ScalarKey::Unixtime(value.to_native())
        }
    }
}

fn number_from_archived(value: &rkyv::Archived<PostingNumberKey>) -> Number {
    match value {
        rkyv::Archived::<PostingNumberKey>::Integer(value) => Number::Integer(value.to_native()),
        rkyv::Archived::<PostingNumberKey>::Float(value) => Number::Float(value.to_native()),
        rkyv::Archived::<PostingNumberKey>::ExactDecimal {
            negative,
            digits,
            exponent,
        } => Number::ExactDecimal(crate::decimal::ExactDecimal {
            negative: bool::from(*negative),
            digits: digits.as_str().into(),
            exponent: exponent.to_native(),
        }),
    }
}

impl PostingDictionaryKey for StructuralKey {
    type Query = StructuralKey;
    type Public = StructuralKey;

    fn stored_query(query: &Self::Query) -> Self {
        *query
    }

    fn stored_from_public(value: &Self::Public) -> Self {
        *value
    }

    fn stored_from_public_owned(value: Self::Public) -> Self {
        value
    }

    fn compare_query(query: &Self::Query, archived: &Self::Archived) -> Ordering {
        structural_key_index(*query).cmp(&structural_key_index(structural_key_from_archived(
            archived,
        )))
    }

    fn public_from_stored(value: &Self) -> Self::Public {
        *value
    }

    fn public_from_archived(value: &Self::Archived) -> Self::Public {
        structural_key_from_archived(value)
    }

    fn empty_published() -> &'static PublishedPostingDictionary<Self> {
        static EMPTY: OnceLock<PublishedPostingDictionary<StructuralKey>> = OnceLock::new();
        EMPTY.get_or_init(empty_structural_published_dictionary)
    }
}

#[derive(Archive, ArchiveSerialize)]
struct OrderedPostingEntry<K> {
    key: K,
    ordinal: u32,
}

#[derive(Archive, ArchiveSerialize)]
pub(crate) struct OrderedPostingArchive<K> {
    postings: BTreeMap<K, u32>,
    ascending: Vec<OrderedPostingEntry<K>>,
}

struct OrderedPostingMapResolver {
    postings: BTreeMapResolver,
    ascending: VecResolver,
}

struct OrderedPostingArchiveSource<'a, K> {
    postings: &'a BTreeMap<K, u32>,
}

struct BorrowedOrderedPostingEntry<'a, K> {
    key: &'a K,
    ordinal: u32,
}

struct BorrowedOrderedPostingEntryResolver<K>
where
    K: Archive,
{
    key: K::Resolver,
    ordinal: <u32 as Archive>::Resolver,
}

impl<K> Archive for BorrowedOrderedPostingEntry<'_, K>
where
    K: Archive,
{
    type Archived = Archived<OrderedPostingEntry<K>>;
    type Resolver = BorrowedOrderedPostingEntryResolver<K>;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: The adapter resolves precisely the key and ordinal fields of
        // the generated archived entry, in declaration order.
        unsafe {
            let root = out.ptr();
            let key = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).key));
            let ordinal = Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).ordinal));
            Archive::resolve(self.key, resolver.key, key);
            Archive::resolve(&self.ordinal, resolver.ordinal, ordinal);
        }
    }
}

impl<K, S> ArchiveSerialize<S> for BorrowedOrderedPostingEntry<'_, K>
where
    K: ArchiveSerialize<S> + Archive,
    S: rkyv::rancor::Fallible + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        Ok(BorrowedOrderedPostingEntryResolver {
            key: ArchiveSerialize::serialize(self.key, serializer)?,
            ordinal: ArchiveSerialize::serialize(&self.ordinal, serializer)?,
        })
    }
}

impl<K> Archive for OrderedPostingArchiveSource<'_, K>
where
    K: Archive + Ord,
    K::Archived: Ord,
{
    type Archived = Archived<OrderedPostingArchive<K>>;
    type Resolver = OrderedPostingMapResolver;

    fn resolve(&self, resolver: Self::Resolver, out: Place<Self::Archived>) {
        // SAFETY: `OrderedPostingArchiveSource` resolves exactly the two
        // fields of `OrderedPostingArchive`, in declaration order, into the
        // matching generated archived type. The field pointers come from the
        // destination place and are used only to initialize those fields.
        unsafe {
            let root = out.ptr();
            let postings =
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).postings));
            let ascending =
                Place::from_field_unchecked(out, std::ptr::addr_of_mut!((*root).ascending));
            ArchivedBTreeMap::<K::Archived, Archived<u32>, 5>::resolve_from_len(
                self.postings.len(),
                resolver.postings,
                postings,
            );
            ArchivedVec::resolve_from_len(self.postings.len(), resolver.ascending, ascending);
        }
    }
}

impl<K, S> ArchiveSerialize<S> for OrderedPostingArchiveSource<'_, K>
where
    K: ArchiveSerialize<S> + Archive + Ord,
    K::Archived: Ord,
    S: rkyv::rancor::Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    S::Error: rkyv::rancor::Source,
{
    fn serialize(&self, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let postings =
            ArchivedBTreeMap::<K::Archived, Archived<u32>, 5>::serialize_from_ordered_iter::<
                _,
                &K,
                &u32,
                K,
                u32,
                S,
            >(self.postings.iter(), serializer)?;
        let ascending = ArchivedVec::serialize_from_iter::<_, _, _>(
            self.postings
                .iter()
                .map(|(key, ordinal)| BorrowedOrderedPostingEntry {
                    key,
                    ordinal: *ordinal,
                }),
            serializer,
        )?;
        Ok(OrderedPostingMapResolver {
            postings,
            ascending,
        })
    }
}

#[derive(Clone)]
struct NativePostingBuilder<K> {
    values: BTreeMap<K, u32>,
    bitmaps: Vec<RoaringBitmap>,
}

enum PublishedPostingBacking<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    Native(Arc<BTreeMap<K, u32>>),
    Archived(Arc<ValidatedArchive<OrderedPostingArchive<K>>>),
    Stored(StoredPostingBacking<K>),
}

pub(crate) struct StoredPostingBacking<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    values: Arc<BTreeMap<K, u32>>,
    leaves: Arc<Vec<StoredPostingLeafRef>>,
    loader: Arc<dyn PostingLeafLoader>,
    decoded: Arc<Vec<OnceLock<Arc<RoaringBitmap>>>>,
    loads: Arc<Vec<std::sync::Mutex<()>>>,
}

impl<K> Clone for StoredPostingBacking<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn clone(&self) -> Self {
        Self {
            values: Arc::clone(&self.values),
            leaves: Arc::clone(&self.leaves),
            loader: Arc::clone(&self.loader),
            decoded: Arc::clone(&self.decoded),
            loads: Arc::clone(&self.loads),
        }
    }
}

impl<K> StoredPostingBacking<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn load_ordinal(&self, ordinal: usize) -> Result<&RoaringBitmap, String> {
        let cell = self
            .decoded
            .get(ordinal)
            .ok_or("stored posting ordinal is out of bounds")?;
        if cell.get().is_none() {
            let _guard = self
                .loads
                .get(ordinal)
                .ok_or("stored posting load lock is missing")?
                .lock()
                .map_err(|_| "stored posting load lock poisoned")?;
            if cell.get().is_some() {
                return Ok(cell
                    .get()
                    .expect("stored posting leaf was initialized")
                    .as_ref());
            }
            let reference = self
                .leaves
                .get(ordinal)
                .ok_or("stored posting leaf reference is missing")?;
            let bitmap = self.loader.load_posting_leaf(reference)?;
            if bitmap.len() != reference.cardinality {
                return Err("stored posting leaf cardinality mismatch".into());
            }
            let _ = cell.set(Arc::new(bitmap));
        }
        Ok(cell
            .get()
            .expect("stored posting leaf was initialized")
            .as_ref())
    }
}

impl<K> Clone for PublishedPostingBacking<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn clone(&self) -> Self {
        match self {
            Self::Native(values) => Self::Native(Arc::clone(values)),
            Self::Archived(archive) => Self::Archived(Arc::clone(archive)),
            Self::Stored(values) => Self::Stored(values.clone()),
        }
    }
}

pub(crate) struct PublishedPostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    backing: PublishedPostingBacking<K>,
    bitmaps: Arc<Vec<RoaringBitmap>>,
}

fn empty_published_dictionary<K>() -> PublishedPostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    PublishedPostingDictionary {
        backing: PublishedPostingBacking::Native(Arc::new(BTreeMap::new())),
        bitmaps: Arc::new(Vec::new()),
    }
}

fn empty_scalar_published_dictionary() -> PublishedPostingDictionary<StoredScalarKey> {
    empty_published_dictionary()
}

fn empty_number_published_dictionary() -> PublishedPostingDictionary<PostingNumberKey> {
    empty_published_dictionary()
}

fn empty_structural_published_dictionary() -> PublishedPostingDictionary<StructuralKey> {
    empty_published_dictionary()
}

impl<K> Clone for PublishedPostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn clone(&self) -> Self {
        Self {
            backing: self.backing.clone(),
            bitmaps: Arc::clone(&self.bitmaps),
        }
    }
}

impl<K> PublishedPostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn from_stored(
        entries: BTreeMap<K, StoredPostingLeafRef>,
        loader: Arc<dyn PostingLeafLoader>,
    ) -> Self {
        let mut values = BTreeMap::new();
        let mut leaves = Vec::with_capacity(entries.len());
        for (key, reference) in entries {
            values.insert(key, leaves.len() as u32);
            leaves.push(reference);
        }
        let leaf_count = leaves.len();
        let decoded = (0..leaf_count).map(|_| OnceLock::new()).collect::<Vec<_>>();
        Self {
            backing: PublishedPostingBacking::Stored(StoredPostingBacking {
                values: Arc::new(values),
                leaves: Arc::new(leaves),
                loader,
                decoded: Arc::new(decoded),
                loads: Arc::new((0..leaf_count).map(|_| std::sync::Mutex::new(())).collect()),
            }),
            bitmaps: Arc::new(Vec::new()),
        }
    }

    fn representation_name(&self) -> &'static str {
        match self.backing {
            PublishedPostingBacking::Native(_) => "native",
            PublishedPostingBacking::Archived(_) => "archived",
            PublishedPostingBacking::Stored(_) => "stored",
        }
    }

    fn len(&self) -> usize {
        match &self.backing {
            PublishedPostingBacking::Native(values) => values.len(),
            PublishedPostingBacking::Archived(archive) => archive.root().postings.len(),
            PublishedPostingBacking::Stored(stored) => stored.values.len(),
        }
    }

    fn archive_bytes(&self) -> usize {
        match &self.backing {
            PublishedPostingBacking::Native(_) => 0,
            PublishedPostingBacking::Archived(archive) => archive.len(),
            PublishedPostingBacking::Stored(_) => 0,
        }
    }

    fn first_public(&self) -> Option<K::Public> {
        match &self.backing {
            PublishedPostingBacking::Native(values) => {
                values.keys().next().map(K::public_from_stored)
            }
            PublishedPostingBacking::Archived(archive) => archive
                .root()
                .ascending
                .first()
                .map(|entry| K::public_from_archived(&entry.key)),
            PublishedPostingBacking::Stored(stored) => {
                stored.values.keys().next().map(K::public_from_stored)
            }
        }
    }

    fn last_public(&self) -> Option<K::Public> {
        match &self.backing {
            PublishedPostingBacking::Native(values) => {
                values.keys().next_back().map(K::public_from_stored)
            }
            PublishedPostingBacking::Archived(archive) => archive
                .root()
                .ascending
                .last()
                .map(|entry| K::public_from_archived(&entry.key)),
            PublishedPostingBacking::Stored(stored) => {
                stored.values.keys().next_back().map(K::public_from_stored)
            }
        }
    }

    fn get(&self, query: &K::Query) -> Option<&RoaringBitmap> {
        self.get_checked(query).ok().flatten()
    }

    fn get_checked(&self, query: &K::Query) -> Result<Option<&RoaringBitmap>, String> {
        let ordinal = match &self.backing {
            PublishedPostingBacking::Native(values) => {
                let query = K::stored_query(query);
                let Some(ordinal) = values.get(&query) else {
                    return Ok(None);
                };
                *ordinal
            }
            PublishedPostingBacking::Archived(archive) => {
                let Some((_, ordinal)) = archive
                    .root()
                    .postings
                    .get_key_value_with(query, K::compare_query)
                else {
                    return Ok(None);
                };
                ordinal.to_native()
            }
            PublishedPostingBacking::Stored(stored) => {
                let query = K::stored_query(query);
                let Some(ordinal) = stored.values.get(&query).copied() else {
                    return Ok(None);
                };
                return stored.load_ordinal(ordinal as usize).map(Some);
            }
        };
        Ok(self.bitmaps.get(ordinal as usize))
    }

    fn cardinality(&self, query: &K::Query) -> u64 {
        match &self.backing {
            PublishedPostingBacking::Stored(stored) => {
                let query = K::stored_query(query);
                stored
                    .values
                    .get(&query)
                    .and_then(|ordinal| stored.leaves.get(*ordinal as usize))
                    .map_or(0, |reference| reference.cardinality)
            }
            _ => self.get(query).map_or(0, RoaringBitmap::len),
        }
    }

    fn to_native_map(&self) -> Result<BTreeMap<K, u32>, String> {
        match &self.backing {
            PublishedPostingBacking::Native(values) => Ok(values.as_ref().clone()),
            PublishedPostingBacking::Archived(archive) => {
                let mut values = BTreeMap::new();
                for entry in archive.root().ascending.iter() {
                    let public = K::public_from_archived(&entry.key);
                    values.insert(K::stored_from_public(&public), entry.ordinal.to_native());
                }
                Ok(values)
            }
            PublishedPostingBacking::Stored(stored) => Ok(stored.values.as_ref().clone()),
        }
    }
}

impl<K> fmt::Debug for PublishedPostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublishedPostingDictionary")
            .field("entries", &self.len())
            .field("representation", &self.representation_name())
            .field("archive_bytes", &self.archive_bytes())
            .finish()
    }
}

enum PostingDictionaryState<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    Builder(NativePostingBuilder<K>),
    Published(PublishedPostingDictionary<K>),
}

/// A structured posting dictionary with a mutable construction phase and an
/// immutable checked archive phase.
pub(crate) struct PostingDictionary<K>(PostingDictionaryState<K>)
where
    K: PostingDictionaryKey,
    K::Archived: Ord;

impl<K> Default for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn default() -> Self {
        Self(PostingDictionaryState::Builder(NativePostingBuilder {
            values: BTreeMap::new(),
            bitmaps: Vec::new(),
        }))
    }
}

impl<K> Clone for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn clone(&self) -> Self {
        Self(match &self.0 {
            PostingDictionaryState::Builder(values) => {
                PostingDictionaryState::Builder(values.clone())
            }
            PostingDictionaryState::Published(values) => {
                PostingDictionaryState::Published(values.clone())
            }
        })
    }
}

impl<K> PartialEq for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut left = self.range(Unbounded, Unbounded, false);
        let mut right = other.range(Unbounded, Unbounded, false);
        loop {
            match (left.next(), right.next()) {
                (None, None) => return true,
                (Some((left_key, left_posting)), Some((right_key, right_posting))) => {
                    if left_key != right_key || left_posting != right_posting {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

impl<K> Eq for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
}

impl<K> fmt::Debug for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.0 {
            PostingDictionaryState::Builder(_) => "builder",
            PostingDictionaryState::Published(values) => values.representation_name(),
        };
        formatter
            .debug_struct("PostingDictionary")
            .field("state", &state)
            .finish()
    }
}

pub(crate) enum DictionaryValueCursor<'a, K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    Empty,
    Builder {
        range: std::collections::btree_map::Range<'a, K, u32>,
        bitmaps: &'a [RoaringBitmap],
        reverse: bool,
    },
    Native {
        range: std::collections::btree_map::Range<'a, K, u32>,
        bitmaps: &'a [RoaringBitmap],
        reverse: bool,
    },
    Archived {
        range: rkyv::collections::btree_map::Range<'a, K::Archived, rkyv::rend::u32_le, 5>,
        bitmaps: &'a [RoaringBitmap],
    },
    ArchivedReverse {
        archive: &'a ValidatedArchive<OrderedPostingArchive<K>>,
        bitmaps: &'a [RoaringBitmap],
        next: Option<usize>,
        lower: usize,
    },
    Stored {
        range: std::collections::btree_map::Range<'a, K, u32>,
        stored: &'a StoredPostingBacking<K>,
        reverse: bool,
    },
}

impl<'a, K> DictionaryValueCursor<'a, K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    pub(crate) fn next(&mut self) -> Option<(K::Public, &'a RoaringBitmap)> {
        match self {
            Self::Empty => None,
            Self::Builder {
                range,
                bitmaps,
                reverse,
            }
            | Self::Native {
                range,
                bitmaps,
                reverse,
            } => {
                let (key, ordinal) = if *reverse {
                    range.next_back()
                } else {
                    range.next()
                }?;
                let posting = bitmaps.get(*ordinal as usize)?;
                Some((K::public_from_stored(key), posting))
            }
            Self::Archived { range, bitmaps } => range.next().and_then(|(key, ordinal)| {
                let posting = bitmaps.get(ordinal.to_native() as usize)?;
                Some((K::public_from_archived(key), posting))
            }),
            Self::ArchivedReverse {
                archive,
                bitmaps,
                next,
                lower,
            } => {
                let index = (*next)?;
                let entry = archive.root().ascending.get(index)?;
                let posting = bitmaps.get(entry.ordinal.to_native() as usize)?;
                *next = index.checked_sub(1).filter(|next| *next >= *lower);
                Some((K::public_from_archived(&entry.key), posting))
            }
            Self::Stored {
                range,
                stored,
                reverse,
            } => {
                let (key, ordinal) = if *reverse {
                    range.next_back()
                } else {
                    range.next()
                }?;
                let posting = stored.load_ordinal(*ordinal as usize).ok()?;
                Some((K::public_from_stored(key), posting))
            }
        }
    }
}

impl<K> PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    fn from_published(values: PublishedPostingDictionary<K>) -> Self {
        Self(PostingDictionaryState::Published(values))
    }

    fn is_stored(&self) -> bool {
        matches!(
            &self.0,
            PostingDictionaryState::Published(PublishedPostingDictionary {
                backing: PublishedPostingBacking::Stored(_),
                ..
            })
        )
    }

    pub(crate) fn builder() -> Self {
        Self::default()
    }

    fn from_native_builder(builder: NativePostingBuilder<K>) -> Self {
        Self(PostingDictionaryState::Builder(builder))
    }

    pub(crate) fn from_public(
        entries: impl IntoIterator<Item = (K::Public, RoaringBitmap)>,
    ) -> Self {
        let mut dictionary = Self::builder();
        for (key, posting) in entries {
            dictionary
                .insert(K::stored_from_public_owned(key), posting)
                .expect("new public posting dictionary has room");
        }
        dictionary
    }

    pub(crate) fn from_stored(
        entries: BTreeMap<K::Public, StoredPostingLeafRef>,
        loader: Arc<dyn PostingLeafLoader>,
    ) -> Self {
        let entries = entries
            .into_iter()
            .map(|(key, reference)| (K::stored_from_public_owned(key), reference))
            .collect();
        Self::from_published(PublishedPostingDictionary::from_stored(entries, loader))
    }

    fn builder_parts_mut(&mut self) -> &mut NativePostingBuilder<K> {
        match &mut self.0 {
            PostingDictionaryState::Builder(builder) => builder,
            PostingDictionaryState::Published(_) => {
                panic!("published posting dictionary cannot be mutated")
            }
        }
    }

    fn into_native_builder(self) -> Result<NativePostingBuilder<K>, String> {
        match self.0 {
            PostingDictionaryState::Builder(builder) => Ok(builder),
            PostingDictionaryState::Published(values) => Ok(NativePostingBuilder {
                values: values.to_native_map()?,
                bitmaps: (*values.bitmaps).clone(),
            }),
        }
    }

    pub(crate) fn is_archived(&self) -> bool {
        matches!(
            self.0,
            PostingDictionaryState::Published(PublishedPostingDictionary {
                backing: PublishedPostingBacking::Archived(_),
                ..
            })
        )
    }

    pub(crate) fn is_storage_backed(&self) -> bool {
        self.is_stored()
    }

    #[allow(dead_code)]
    pub(crate) fn is_native(&self) -> bool {
        matches!(
            self.0,
            PostingDictionaryState::Published(PublishedPostingDictionary {
                backing: PublishedPostingBacking::Native(_),
                ..
            })
        )
    }

    pub(crate) fn is_published(&self) -> bool {
        matches!(self.0, PostingDictionaryState::Published(_))
    }

    fn first_public(&self) -> Option<K::Public> {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                builder.values.keys().next().map(K::public_from_stored)
            }
            PostingDictionaryState::Published(values) => values.first_public(),
        }
    }

    fn last_public(&self) -> Option<K::Public> {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                builder.values.keys().next_back().map(K::public_from_stored)
            }
            PostingDictionaryState::Published(values) => values.last_public(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => builder.values.len(),
            PostingDictionaryState::Published(values) => values.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn get(&self, query: &K::Query) -> Option<&RoaringBitmap> {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                let query = K::stored_query(query);
                let ordinal = *builder.values.get(&query)?;
                builder.bitmaps.get(ordinal as usize)
            }
            PostingDictionaryState::Published(values) => values.get(query),
        }
    }

    pub(crate) fn get_checked(&self, query: &K::Query) -> Result<Option<&RoaringBitmap>, String> {
        match &self.0 {
            PostingDictionaryState::Published(values) => values.get_checked(query),
            _ => Ok(self.get(query)),
        }
    }

    pub(crate) fn cardinality(&self, query: &K::Query) -> u64 {
        match &self.0 {
            PostingDictionaryState::Published(values) => values.cardinality(query),
            _ => self.get(query).map_or(0, RoaringBitmap::len),
        }
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, query: &K::Query) -> bool {
        self.get(query).is_some()
    }

    pub(crate) fn range(
        &self,
        lower: std::ops::Bound<K::Query>,
        upper: std::ops::Bound<K::Query>,
        descending: bool,
    ) -> DictionaryValueCursor<'_, K> {
        if invalid_query_bounds::<K>(&lower, &upper) {
            return DictionaryValueCursor::Empty;
        }
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                let lower = lower.map(|query| K::stored_query(&query));
                let upper = upper.map(|query| K::stored_query(&query));
                let bounds = (lower, upper);
                DictionaryValueCursor::Builder {
                    range: builder.values.range(bounds),
                    bitmaps: &builder.bitmaps,
                    reverse: descending,
                }
            }
            PostingDictionaryState::Published(values) => match &values.backing {
                PublishedPostingBacking::Native(map) => {
                    let lower = lower.map(|query| K::stored_query(&query));
                    let upper = upper.map(|query| K::stored_query(&query));
                    DictionaryValueCursor::Native {
                        range: map.range((lower, upper)),
                        bitmaps: &values.bitmaps,
                        reverse: descending,
                    }
                }
                PublishedPostingBacking::Archived(archive) if !descending => {
                    let range = archive
                        .root()
                        .postings
                        .range_with((lower, upper), K::compare_query);
                    DictionaryValueCursor::Archived {
                        range,
                        bitmaps: &values.bitmaps,
                    }
                }
                PublishedPostingBacking::Archived(archive) => {
                    let (lower, upper) =
                        archived_reverse_bounds(&archive.root().ascending, &lower, &upper);
                    DictionaryValueCursor::ArchivedReverse {
                        archive,
                        bitmaps: &values.bitmaps,
                        next: upper.checked_sub(1).filter(|index| *index >= lower),
                        lower,
                    }
                }
                PublishedPostingBacking::Stored(stored) => {
                    let lower = lower.map(|query| K::stored_query(&query));
                    let upper = upper.map(|query| K::stored_query(&query));
                    DictionaryValueCursor::Stored {
                        range: stored.values.range((lower, upper)),
                        stored,
                        reverse: descending,
                    }
                }
            },
        }
    }

    pub(crate) fn for_each(
        &self,
        mut visit: impl FnMut(&K::Public, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                for (key, ordinal) in &builder.values {
                    let posting = builder
                        .bitmaps
                        .get(*ordinal as usize)
                        .ok_or("posting bitmap slot out of bounds")?;
                    visit(&K::public_from_stored(key), posting)?;
                }
            }
            PostingDictionaryState::Published(values) => match &values.backing {
                PublishedPostingBacking::Native(map) => {
                    for (key, ordinal) in map.iter() {
                        let posting = values
                            .bitmaps
                            .get(*ordinal as usize)
                            .ok_or("native posting bitmap slot out of bounds")?;
                        visit(&K::public_from_stored(key), posting)?;
                    }
                }
                PublishedPostingBacking::Archived(archive) => {
                    for entry in archive.root().ascending.iter() {
                        let posting = values
                            .bitmaps
                            .get(entry.ordinal.to_native() as usize)
                            .ok_or("archived posting ordinal out of bounds")?;
                        visit(&K::public_from_archived(&entry.key), posting)?;
                    }
                }
                PublishedPostingBacking::Stored(stored) => {
                    for (key, ordinal) in stored.values.iter() {
                        let posting = stored.load_ordinal(*ordinal as usize)?;
                        visit(&K::public_from_stored(key), posting)?;
                    }
                }
            },
        }
        Ok(())
    }

    pub(crate) fn for_each_bitmap(
        &self,
        mut visit: impl FnMut(&RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => {
                for posting in &builder.bitmaps {
                    visit(posting)?;
                }
            }
            PostingDictionaryState::Published(values) => match &values.backing {
                PublishedPostingBacking::Stored(stored) => {
                    for ordinal in 0..stored.leaves.len() {
                        visit(stored.load_ordinal(ordinal)?)?;
                    }
                }
                _ => {
                    for posting in values.bitmaps.iter() {
                        visit(posting)?;
                    }
                }
            },
        }
        Ok(())
    }

    pub(crate) fn iter(&self) -> std::vec::IntoIter<(K::Public, RoaringBitmap)> {
        let mut values = Vec::with_capacity(self.len());
        let mut cursor = self.range(Unbounded, Unbounded, false);
        while let Some((key, posting)) = cursor.next() {
            values.push((key, posting.clone()));
        }
        values.into_iter()
    }

    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = RoaringBitmap> + '_ {
        self.iter().map(|(_, posting)| posting)
    }

    /// Publish the native builder as immutable shared ownership without
    /// serializing it. Empty postings are removed while their now-unreferenced
    /// payload slots remain empty, preserving every live insertion
    /// identifier.
    pub(crate) fn publish_native(mut self) -> Result<Self, String>
    where
        K: 'static,
    {
        let mut builder = match std::mem::replace(
            &mut self.0,
            PostingDictionaryState::Builder(NativePostingBuilder {
                values: BTreeMap::new(),
                bitmaps: Vec::new(),
            }),
        ) {
            PostingDictionaryState::Builder(builder) => builder,
            published @ PostingDictionaryState::Published(_) => {
                self.0 = published;
                return Ok(self);
            }
        };
        let mut removed = Vec::new();
        builder.values.retain(|_, ordinal| {
            let keep = builder
                .bitmaps
                .get(*ordinal as usize)
                .is_some_and(|bitmap| !bitmap.is_empty());
            if !keep {
                removed.push(*ordinal as usize);
            }
            keep
        });
        for ordinal in removed {
            if let Some(bitmap) = builder.bitmaps.get_mut(ordinal) {
                bitmap.clear();
            }
        }
        if builder.values.is_empty() {
            return Ok(Self(PostingDictionaryState::Published(
                K::empty_published().clone(),
            )));
        }
        let published = PublishedPostingDictionary {
            backing: PublishedPostingBacking::Native(Arc::new(builder.values)),
            bitmaps: Arc::new(builder.bitmaps),
        };
        validate_published_dictionary::<K>(&published)?;
        Ok(Self(PostingDictionaryState::Published(published)))
    }

    /// Prepare a checked archived dictionary while borrowing the immutable
    /// published source. The caller installs the returned owner only after all
    /// component conversions succeed.
    pub(crate) fn archived(&self) -> Result<Self, String> {
        match &self.0 {
            PostingDictionaryState::Published(PublishedPostingDictionary {
                backing: PublishedPostingBacking::Native(map),
                bitmaps,
            }) => {
                let source = OrderedPostingArchiveSource {
                    postings: map.as_ref(),
                };
                let archive = ValidatedArchive::<OrderedPostingArchive<K>>::freeze_source(&source)?;
                let published = PublishedPostingDictionary {
                    backing: PublishedPostingBacking::Archived(archive),
                    bitmaps: Arc::clone(bitmaps),
                };
                validate_published_dictionary::<K>(&published)?;
                Ok(Self(PostingDictionaryState::Published(published)))
            }
            PostingDictionaryState::Published(_) => Ok(self.clone()),
            PostingDictionaryState::Builder(_) => {
                Err("posting dictionary must be published before archiving".into())
            }
        }
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        match &self.0 {
            PostingDictionaryState::Builder(_) => 0,
            PostingDictionaryState::Published(values) => values.archive_bytes(),
        }
    }

    #[cfg(test)]
    pub(crate) fn bitmap_owner_id(&self) -> usize {
        match &self.0 {
            PostingDictionaryState::Builder(builder) => builder.bitmaps.as_ptr() as usize,
            PostingDictionaryState::Published(values) => Arc::as_ptr(&values.bitmaps) as usize,
        }
    }

    pub(crate) fn for_each_archive_owner(&self, visit: &mut impl FnMut(usize, usize, usize)) {
        if let PostingDictionaryState::Published(values) = &self.0
            && let PublishedPostingBacking::Archived(archive) = &values.backing
        {
            visit(
                Arc::as_ptr(archive) as usize,
                archive.len(),
                archive.capacity(),
            );
        }
    }

    pub(crate) fn clear(&mut self) {
        let builder = self.builder_parts_mut();
        builder.values.clear();
        builder.bitmaps.clear();
    }

    pub(crate) fn insert(
        &mut self,
        key: K,
        value: RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        let builder = self.builder_parts_mut();
        if let Some(ordinal) = builder.values.get(&key).copied() {
            return Ok(builder
                .bitmaps
                .get_mut(ordinal as usize)
                .map(|previous| std::mem::replace(previous, value)));
        }
        let ordinal =
            u32::try_from(builder.bitmaps.len()).map_err(|_| "posting bitmap slot overflow")?;
        builder.values.insert(key, ordinal);
        builder.bitmaps.push(value);
        Ok(None)
    }

    pub(crate) fn insert_public(
        &mut self,
        key: &K::Public,
        value: RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        self.insert(K::stored_from_public(key), value)
    }

    pub(crate) fn remove(&mut self, key: &K) -> Option<RoaringBitmap> {
        let builder = self.builder_parts_mut();
        let ordinal = builder.values.remove(key)?;
        builder
            .bitmaps
            .get_mut(ordinal as usize)
            .map(std::mem::take)
    }

    pub(crate) fn remove_public(&mut self, key: &K::Public) -> Option<RoaringBitmap> {
        self.remove(&K::stored_from_public(key))
    }

    pub(crate) fn bitmap_mut(&mut self, key: K) -> Result<&mut RoaringBitmap, String> {
        let builder = self.builder_parts_mut();
        let ordinal = match builder.values.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let ordinal = u32::try_from(builder.bitmaps.len())
                    .map_err(|_| "posting bitmap slot overflow")?;
                entry.insert(ordinal);
                builder.bitmaps.push(RoaringBitmap::new());
                ordinal
            }
        };
        builder
            .bitmaps
            .get_mut(ordinal as usize)
            .ok_or_else(|| "posting bitmap slot out of bounds".into())
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K::Public, &RoaringBitmap) -> bool) {
        let builder = self.builder_parts_mut();
        let mut removed = Vec::new();
        builder.values.retain(|key, ordinal| {
            let Some(posting) = builder.bitmaps.get(*ordinal as usize) else {
                removed.push(*ordinal as usize);
                return false;
            };
            let public = K::public_from_stored(key);
            let retain = keep(&public, posting);
            if !retain {
                removed.push(*ordinal as usize);
            }
            retain
        });
        for ordinal in removed {
            if let Some(posting) = builder.bitmaps.get_mut(ordinal) {
                posting.clear();
            }
        }
    }
}

fn invalid_query_bounds<K>(
    lower: &std::ops::Bound<K::Query>,
    upper: &std::ops::Bound<K::Query>,
) -> bool
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    match (lower, upper) {
        (std::ops::Bound::Included(lower), std::ops::Bound::Included(upper)) => lower > upper,
        (
            std::ops::Bound::Included(lower) | std::ops::Bound::Excluded(lower),
            std::ops::Bound::Excluded(upper),
        ) => lower >= upper,
        (std::ops::Bound::Excluded(lower), std::ops::Bound::Included(upper)) => lower >= upper,
        _ => false,
    }
}

impl<'dict, K> IntoIterator for &'dict PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    type Item = (K::Public, RoaringBitmap);
    type IntoIter = std::vec::IntoIter<(K::Public, RoaringBitmap)>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K> std::ops::Index<&K::Query> for PostingDictionary<K>
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    type Output = RoaringBitmap;

    fn index(&self, query: &K::Query) -> &Self::Output {
        self.get(query)
            .expect("posting dictionary query key must exist")
    }
}

fn archived_reverse_bounds<K>(
    entries: &[rkyv::Archived<OrderedPostingEntry<K>>],
    lower: &std::ops::Bound<K::Query>,
    upper: &std::ops::Bound<K::Query>,
) -> (usize, usize)
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    let start = match lower {
        std::ops::Bound::Unbounded => 0,
        std::ops::Bound::Included(query) => entries
            .partition_point(|entry| K::compare_query(query, &entry.key) == Ordering::Greater),
        std::ops::Bound::Excluded(query) => {
            entries.partition_point(|entry| K::compare_query(query, &entry.key) != Ordering::Less)
        }
    };
    let end = match upper {
        std::ops::Bound::Unbounded => entries.len(),
        std::ops::Bound::Included(query) => {
            entries.partition_point(|entry| K::compare_query(query, &entry.key) != Ordering::Less)
        }
        std::ops::Bound::Excluded(query) => entries
            .partition_point(|entry| K::compare_query(query, &entry.key) == Ordering::Greater),
    };
    (start.min(entries.len()), end.min(entries.len()))
}

fn validate_published_dictionary<K>(
    dictionary: &PublishedPostingDictionary<K>,
) -> Result<(), String>
where
    K: PostingDictionaryKey,
    K::Archived: Ord,
{
    let mut referenced = vec![false; dictionary.bitmaps.len()];
    let visits = match &dictionary.backing {
        PublishedPostingBacking::Native(values) => {
            for ordinal in values.values().copied() {
                let Some(posting) = dictionary.bitmaps.get(ordinal as usize) else {
                    return Err("native posting bitmap slot out of bounds".into());
                };
                if posting.is_empty() || std::mem::replace(&mut referenced[ordinal as usize], true)
                {
                    return Err("native posting slot is empty or multiply referenced".into());
                }
            }
            values.len()
        }
        PublishedPostingBacking::Archived(archive) => {
            let root = archive.root();
            if root.postings.len() != root.ascending.len() {
                return Err("archived posting dictionary length mismatch".into());
            }
            let mut tree = root.postings.iter();
            let mut previous: Option<&K::Archived> = None;
            for entry in root.ascending.iter() {
                if previous.is_some_and(|previous| previous >= &entry.key) {
                    return Err("archived posting dictionary is not strictly ordered".into());
                }
                let ordinal = entry.ordinal.to_native() as usize;
                let Some(posting) = dictionary.bitmaps.get(ordinal) else {
                    return Err("archived posting bitmap slot out of bounds".into());
                };
                if posting.is_empty() || std::mem::replace(&mut referenced[ordinal], true) {
                    return Err("archived posting slot is empty or multiply referenced".into());
                }
                let (tree_key, tree_ordinal) = tree
                    .next()
                    .ok_or("archived reverse entry is absent from the tree")?;
                if tree_key != &entry.key || tree_ordinal.to_native() as usize != ordinal {
                    return Err("archived posting reverse directory disagrees with the tree".into());
                }
                previous = Some(&entry.key);
            }
            if tree.next().is_some() {
                return Err("archived posting tree traversal length mismatch".into());
            }
            root.ascending.len()
        }
        PublishedPostingBacking::Stored(stored) => {
            if stored.values.len() != stored.leaves.len()
                || stored.leaves.len() != stored.decoded.len()
            {
                return Err("stored posting dictionary and leaf directory lengths differ".into());
            }
            for ordinal in stored.values.values().copied() {
                if ordinal as usize >= stored.leaves.len() {
                    return Err("stored posting leaf ordinal is out of bounds".into());
                }
            }
            for ordinal in 0..stored.leaves.len() {
                if stored.load_ordinal(ordinal)?.is_empty() {
                    return Err("stored posting leaf is empty".into());
                }
            }
            stored.values.len()
        }
    };
    if dictionary
        .bitmaps
        .iter()
        .zip(referenced)
        .any(|(posting, referenced)| !referenced && !posting.is_empty())
    {
        return Err("posting dictionary retains a nonempty unreachable payload".into());
    }
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.semantic_archive_visits = counters
            .semantic_archive_visits
            .saturating_add(visits as u64);
    });
    Ok(())
}

impl ScalarKey {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::String(s) => Self::String(s.clone()),
            Value::Id(s) => Self::Id(s.clone()),
            Value::Number(n) => Self::Number(n.clone()),
            Value::Bool(b) => Self::Bool(*b),
            Value::Date(d) => Self::Date(*d),
            Value::Datetime(t) => Self::Datetime(*t),
            Value::Unixtime(t) => Self::Unixtime(*t),
            _ => return None,
        })
    }
}

/// Ordered typed values and their record-slot postings.
#[derive(Clone, Debug)]
pub struct PostingIndex {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: RoaringBitmap,
    /// Legacy path presence can include a traversal that selects no values.
    /// Only these exceptional records need a separate bitmap for `:exists`.
    pub empty_selections: RoaringBitmap,
    pub nulls: RoaringBitmap,
    /// Exact selected-occurrence facts for an admitted logical field. `None`
    /// preserves the narrower legacy structural-only or named value index.
    pub structural: Option<StructuralFacts>,
    pub(crate) values: PostingDictionary<StoredScalarKey>,
    pub(crate) lengths: PostingDictionary<PostingNumberKey>,
    /// Whether the declared field admits a complete typed value dictionary.
    /// Partial length and empty-string shortcuts are tracked separately.
    pub values_indexed: bool,
    /// A partial dictionary cannot prove that an absent scalar value is absent.
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub stats: PostingStats,
    pub(crate) has_structural: bool,
    pub(crate) storage: Option<Arc<dyn PostingIndexLoader>>,
    pub(crate) loaded: Arc<OnceLock<Box<PostingIndex>>>,
    pub(crate) load_lock: Arc<std::sync::Mutex<()>>,
}

/// Sparse facts inside the owning field posting, never synthetic field paths.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Archive,
    ArchiveSerialize,
    ArchiveDeserialize,
    Serialize,
    Deserialize,
)]
pub enum StructuralKey {
    AnyNull,
    NonEmpty,
    String,
    Number,
    Array,
    Object,
    EmptyObject,
}

fn structural_key_index(key: StructuralKey) -> u8 {
    match key {
        StructuralKey::AnyNull => 0,
        StructuralKey::NonEmpty => 1,
        StructuralKey::String => 2,
        StructuralKey::Number => 3,
        StructuralKey::Array => 4,
        StructuralKey::Object => 5,
        StructuralKey::EmptyObject => 6,
    }
}

fn structural_key_from_archived(key: &rkyv::Archived<StructuralKey>) -> StructuralKey {
    match key {
        rkyv::Archived::<StructuralKey>::AnyNull => StructuralKey::AnyNull,
        rkyv::Archived::<StructuralKey>::NonEmpty => StructuralKey::NonEmpty,
        rkyv::Archived::<StructuralKey>::String => StructuralKey::String,
        rkyv::Archived::<StructuralKey>::Number => StructuralKey::Number,
        rkyv::Archived::<StructuralKey>::Array => StructuralKey::Array,
        rkyv::Archived::<StructuralKey>::Object => StructuralKey::Object,
        rkyv::Archived::<StructuralKey>::EmptyObject => StructuralKey::EmptyObject,
    }
}

impl PartialEq for ArchivedStructuralKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ArchivedStructuralKey {}

impl PartialOrd for ArchivedStructuralKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ArchivedStructuralKey {
    fn cmp(&self, other: &Self) -> Ordering {
        structural_key_index(structural_key_from_archived(self))
            .cmp(&structural_key_index(structural_key_from_archived(other)))
    }
}

#[derive(Clone, Debug, Default)]
pub struct StructuralFacts {
    pub(crate) postings: PostingDictionary<StructuralKey>,
}

impl StructuralFacts {
    fn insert(&mut self, key: StructuralKey, slot: RecordSlot) {
        self.postings
            .bitmap_mut(key)
            .expect("posting bitmap slot domain is checked")
            .insert(slot.0);
    }

    fn observe(&mut self, slot: RecordSlot, value: &Value, multi: bool) {
        if value_matches(value, StatePredicate::NonEmpty) {
            self.insert(StructuralKey::NonEmpty, slot);
        }
        match value {
            Value::Null if multi => self.insert(StructuralKey::AnyNull, slot),
            Value::String(_) | Value::Id(_) => self.insert(StructuralKey::String, slot),
            Value::Number(_) => self.insert(StructuralKey::Number, slot),
            Value::Array(_) => self.insert(StructuralKey::Array, slot),
            Value::Object(map) => {
                self.insert(StructuralKey::Object, slot);
                if map.is_empty() {
                    self.insert(StructuralKey::EmptyObject, slot);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        let mut batch = batch.postings.into_native_builder()?;
        for (key, ordinal) in batch.values {
            let posting = batch
                .bitmaps
                .get_mut(ordinal as usize)
                .ok_or("posting bitmap slot out of bounds")?;
            append_shifted_bitmap(self.postings.bitmap_mut(key)?, posting, base)?;
        }
        Ok(())
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.postings.archive_bytes()
    }
}

/// Optional named-binding posting and its exact coverage guarantee.
#[derive(Clone, Debug)]
pub struct CustomPostingIndex {
    pub index: PostingIndex,
    pub complete: bool,
}

/// Coverage and cardinality summary for one posting index.
#[derive(Clone, Debug, Default)]
pub struct PostingStats {
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: usize,
    pub min_key: Option<ScalarKey>,
    pub max_key: Option<ScalarKey>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PostingIndexSnapshot {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: RoaringBitmap,
    pub empty_selections: RoaringBitmap,
    pub nulls: RoaringBitmap,
    pub structural: Option<BTreeMap<StructuralKey, RoaringBitmap>>,
    pub values: BTreeMap<ScalarKey, RoaringBitmap>,
    pub lengths: BTreeMap<Number, RoaringBitmap>,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: usize,
    pub min_key: Option<ScalarKey>,
    pub max_key: Option<ScalarKey>,
    pub has_structural: bool,
    #[serde(default)]
    pub payload: Option<StoredPostingPayloadRef>,
    #[serde(skip)]
    pub stored_leaves: Option<StoredPostingLeafCatalog>,
    #[serde(skip)]
    pub leaf_loader: Option<Arc<dyn PostingLeafLoader>>,
}

#[derive(Clone, Debug)]
pub(crate) struct StoredPostingLeafCatalog {
    pub values: BTreeMap<ScalarKey, StoredPostingLeafRef>,
    pub lengths: BTreeMap<Number, StoredPostingLeafRef>,
    pub structural: BTreeMap<StructuralKey, StoredPostingLeafRef>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct PostingArchivePayload {
    pub path: Vec<String>,
    pub kind: String,
    pub multi: bool,
    pub presence: Vec<u8>,
    pub empty_selections: Vec<u8>,
    pub nulls: Vec<u8>,
    pub structural: Option<Vec<(StructuralKey, StoredPostingLeafRef)>>,
    pub values: Vec<(StoredScalarKey, StoredPostingLeafRef)>,
    pub lengths: Vec<(PostingNumberKey, StoredPostingLeafRef)>,
    pub values_indexed: bool,
    pub value_complete: bool,
    pub length_complete: bool,
    pub empty_string_complete: bool,
    pub covered_records: u32,
    pub present_count: u64,
    pub null_count: u64,
    pub distinct_count: u64,
    pub min_key: Option<StoredScalarKey>,
    pub max_key: Option<StoredScalarKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredPostingPayloadRef {
    pub digest: [u8; 32],
    pub length: u64,
    pub dictionary_offset: u64,
    pub dictionary_length: u64,
    pub dictionary_digest: [u8; 32],
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
        other => return Err(format!("unknown stored logical type {other}")),
    })
}

fn encode_bitmap(bitmap: &RoaringBitmap) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(bitmap.serialized_size());
    bitmap
        .serialize_into(&mut bytes)
        .map_err(|error| format!("bitmap serialization failed: {error}"))?;
    Ok(bytes)
}

fn decode_bitmap(bytes: &[u8]) -> Result<RoaringBitmap, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let bitmap = RoaringBitmap::deserialize_from(&mut cursor)
        .map_err(|error| format!("bitmap decode failed: {error}"))?;
    if cursor.position() != bytes.len() as u64 {
        return Err("bitmap payload has trailing bytes".into());
    }
    Ok(bitmap)
}

impl PostingIndexSnapshot {
    pub(crate) fn archive_payload_with(
        &self,
        mut leaf: impl FnMut(&RoaringBitmap) -> Result<StoredPostingLeafRef, String>,
    ) -> Result<PostingArchivePayload, String> {
        let structural = self
            .structural
            .as_ref()
            .map(|postings| {
                postings
                    .iter()
                    .map(|(key, bitmap)| Ok((*key, leaf(bitmap)?)))
                    .collect::<Result<_, String>>()
            })
            .transpose()?;
        let values = self
            .values
            .iter()
            .map(|(key, bitmap)| Ok((StoredScalarKey::stored_from_public(key), leaf(bitmap)?)))
            .collect::<Result<_, String>>()?;
        let lengths = self
            .lengths
            .iter()
            .map(|(key, bitmap)| {
                Ok((
                    PostingNumberKey::from_number_owned(key.clone()),
                    leaf(bitmap)?,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(PostingArchivePayload {
            path: self.path.clone(),
            kind: encode_logical_type(&self.kind),
            multi: self.multi,
            presence: encode_bitmap(&self.presence)?,
            empty_selections: encode_bitmap(&self.empty_selections)?,
            nulls: encode_bitmap(&self.nulls)?,
            structural,
            values,
            lengths,
            values_indexed: self.values_indexed,
            value_complete: self.value_complete,
            length_complete: self.length_complete,
            empty_string_complete: self.empty_string_complete,
            covered_records: self.covered_records,
            present_count: self.present_count,
            null_count: self.null_count,
            distinct_count: self.distinct_count as u64,
            min_key: self
                .min_key
                .as_ref()
                .map(StoredScalarKey::stored_from_public),
            max_key: self
                .max_key
                .as_ref()
                .map(StoredScalarKey::stored_from_public),
        })
    }

    pub(crate) fn from_stored_payload(
        payload: PostingArchivePayload,
        leaf_loader: Arc<dyn PostingLeafLoader>,
    ) -> Result<Self, String> {
        let has_structural = payload.structural.is_some();
        let structural = payload
            .structural
            .map(|postings| {
                postings
                    .into_iter()
                    .map(|(key, reference)| (key, reference))
                    .collect()
            })
            .unwrap_or_default();
        let values = payload
            .values
            .into_iter()
            .map(|(key, reference)| (StoredScalarKey::public_from_stored(&key), reference))
            .collect();
        let lengths = payload
            .lengths
            .into_iter()
            .map(|(key, reference)| (PostingNumberKey::public_from_stored(&key), reference))
            .collect();
        Ok(Self {
            path: payload.path,
            kind: decode_logical_type(&payload.kind)?,
            multi: payload.multi,
            presence: decode_bitmap(&payload.presence)?,
            empty_selections: decode_bitmap(&payload.empty_selections)?,
            nulls: decode_bitmap(&payload.nulls)?,
            structural: None,
            values: BTreeMap::new(),
            lengths: BTreeMap::new(),
            values_indexed: payload.values_indexed,
            value_complete: payload.value_complete,
            length_complete: payload.length_complete,
            empty_string_complete: payload.empty_string_complete,
            covered_records: payload.covered_records,
            present_count: payload.present_count,
            null_count: payload.null_count,
            distinct_count: payload.distinct_count as usize,
            min_key: payload
                .min_key
                .as_ref()
                .map(StoredScalarKey::public_from_stored),
            max_key: payload
                .max_key
                .as_ref()
                .map(StoredScalarKey::public_from_stored),
            has_structural,
            payload: None,
            stored_leaves: Some(StoredPostingLeafCatalog {
                values,
                lengths,
                structural,
            }),
            leaf_loader: Some(leaf_loader),
        })
    }
}

/// Borrowed ordered traversal over one scalar posting dictionary.
pub(crate) type PostingValueCursor<'a> = DictionaryValueCursor<'a, StoredScalarKey>;

impl PostingIndex {
    pub(crate) fn storage_snapshot(&self) -> Result<PostingIndexSnapshot, String> {
        if !self.is_published() {
            return Err("posting index is not published".into());
        }
        let stored_payload = self
            .storage
            .as_ref()
            .and_then(|loader| loader.stored_payload_reference());
        if stored_payload.is_some()
            && (self.values.is_storage_backed()
                || self.lengths.is_storage_backed()
                || self
                    .structural
                    .as_ref()
                    .is_some_and(|facts| facts.postings.is_storage_backed()))
        {
            return Ok(PostingIndexSnapshot {
                path: self.path.clone(),
                kind: self.kind.clone(),
                multi: self.multi,
                presence: self.presence.clone(),
                empty_selections: self.empty_selections.clone(),
                nulls: self.nulls.clone(),
                structural: None,
                values: BTreeMap::new(),
                lengths: BTreeMap::new(),
                values_indexed: self.values_indexed,
                value_complete: self.value_complete,
                length_complete: self.length_complete,
                empty_string_complete: self.empty_string_complete,
                covered_records: self.covered_records,
                present_count: self.stats.present_count,
                null_count: self.stats.null_count,
                distinct_count: self.stats.distinct_count,
                min_key: self.stats.min_key.clone(),
                max_key: self.stats.max_key.clone(),
                has_structural: self.has_structural,
                payload: stored_payload,
                stored_leaves: None,
                leaf_loader: None,
            });
        }
        let structural = self.structural.as_ref().map(|facts| {
            facts
                .postings
                .iter()
                .map(|(key, bitmap)| (key, bitmap))
                .collect()
        });
        let lengths = self
            .lengths
            .iter()
            .map(|(key, bitmap)| (key, bitmap))
            .collect::<BTreeMap<_, _>>();
        Ok(PostingIndexSnapshot {
            path: self.path.clone(),
            kind: self.kind.clone(),
            multi: self.multi,
            presence: self.presence.clone(),
            empty_selections: self.empty_selections.clone(),
            nulls: self.nulls.clone(),
            structural,
            values: self.values.iter().collect(),
            lengths,
            values_indexed: self.values_indexed,
            value_complete: self.value_complete,
            length_complete: self.length_complete,
            empty_string_complete: self.empty_string_complete,
            covered_records: self.covered_records,
            present_count: self.stats.present_count,
            null_count: self.stats.null_count,
            distinct_count: self.stats.distinct_count,
            min_key: self.stats.min_key.clone(),
            max_key: self.stats.max_key.clone(),
            has_structural: self.has_structural,
            payload: None,
            stored_leaves: None,
            leaf_loader: None,
        })
    }

    pub(crate) fn from_storage_snapshot(
        mut snapshot: PostingIndexSnapshot,
    ) -> Result<Self, String> {
        let has_structural = snapshot.has_structural || snapshot.structural.is_some();
        let stored_catalog = snapshot.stored_leaves.take();
        let leaf_loader = snapshot.leaf_loader.take();
        let (structural, values, lengths) = if let Some(catalog) = stored_catalog {
            let loader = leaf_loader.ok_or("stored posting leaf loader is missing")?;
            let structural = if snapshot.has_structural {
                Some(StructuralFacts {
                    postings: PostingDictionary::from_stored(
                        catalog.structural,
                        Arc::clone(&loader),
                    ),
                })
            } else {
                None
            };
            (
                structural,
                PostingDictionary::from_stored(catalog.values, Arc::clone(&loader)),
                PostingDictionary::from_stored(catalog.lengths, loader),
            )
        } else {
            let structural = snapshot
                .structural
                .map(|postings| {
                    Ok::<_, String>(StructuralFacts {
                        postings: PostingDictionary::from_public(postings).publish_native()?,
                    })
                })
                .transpose()?;
            (
                structural,
                PostingDictionary::from_public(snapshot.values).publish_native()?,
                PostingDictionary::from_public(snapshot.lengths).publish_native()?,
            )
        };
        let index = Self {
            path: snapshot.path,
            kind: snapshot.kind,
            multi: snapshot.multi,
            presence: snapshot.presence,
            empty_selections: snapshot.empty_selections,
            nulls: snapshot.nulls,
            structural,
            values,
            lengths,
            values_indexed: snapshot.values_indexed,
            value_complete: snapshot.value_complete,
            length_complete: snapshot.length_complete,
            empty_string_complete: snapshot.empty_string_complete,
            covered_records: snapshot.covered_records,
            stats: PostingStats {
                present_count: snapshot.present_count,
                null_count: snapshot.null_count,
                distinct_count: snapshot.distinct_count,
                min_key: snapshot.min_key,
                max_key: snapshot.max_key,
            },
            has_structural,
            storage: None,
            loaded: Arc::new(OnceLock::new()),
            load_lock: Arc::new(std::sync::Mutex::new(())),
        };
        Ok(index)
    }

    pub(crate) fn from_stored_metadata(
        snapshot: PostingIndexSnapshot,
        loader: Arc<dyn PostingIndexLoader>,
    ) -> Result<Self, String> {
        let mut index = Self::from_storage_snapshot(snapshot)?;
        index.storage = Some(loader);
        Ok(index)
    }

    pub(crate) fn ensure_storage_loaded(&self) -> Result<(), String> {
        if self.storage.is_none() {
            return Ok(());
        }
        let _guard = self
            .load_lock
            .lock()
            .map_err(|_| "posting index load lock poisoned".to_string())?;
        if self.loaded.get().is_none() {
            let snapshot = self
                .storage
                .as_ref()
                .expect("stored posting index")
                .load_posting_index()?;
            let loaded = Box::new(Self::from_storage_snapshot(snapshot)?);
            self.loaded
                .set(loaded)
                .map_err(|_| "posting index was initialized concurrently".to_string())?;
        }
        Ok(())
    }

    pub(crate) fn ensure_term_leaves(&self, term: &BoundTerm) -> Result<(), String> {
        self.ensure_storage_loaded()?;
        let this = self.payload();
        if let Some(structural) = this.structural.as_ref()
            && let Some(key) = derived_structural_key(term.derived)
        {
            structural.postings.get_checked(&key)?;
        }
        if term.derived == Derived::Length {
            let Value::Number(number) = &term.literal else {
                return Ok(());
            };
            match term.operator {
                Operator::Eq => {
                    this.lengths.get_checked(number)?;
                }
                Operator::Ne => {
                    let mut range = this.lengths.range(Unbounded, Unbounded, false);
                    while range.next().is_some() {}
                }
                Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                    let (lower, upper) = range_bounds(number, term.operator);
                    let mut range = this.lengths.range(lower, upper, false);
                    while range.next().is_some() {}
                }
                _ => {}
            }
            return Ok(());
        }
        let Some(key) = ScalarKey::from_value(&term.literal) else {
            return Ok(());
        };
        if let Some(period) = &term.period {
            if let (Some(start), Some(end)) = (
                ScalarKey::from_value(&period.start),
                ScalarKey::from_value(&period.end),
            ) {
                let mut range = this.values.range(Excluded(start), Excluded(end), false);
                while range.next().is_some() {}
            }
            return Ok(());
        }
        match term.operator {
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                let (lower, upper) = range_bounds(&key, term.operator);
                let mut range = this.values.range(lower, upper, false);
                while range.next().is_some() {}
            }
            _ => {
                this.values.get_checked(&key)?;
            }
        }
        Ok(())
    }

    pub(crate) fn ensure_all_values_loaded(&self) -> Result<(), String> {
        self.ensure_storage_loaded()?;
        self.payload().values.for_each(|_, _| Ok(()))
    }

    pub(crate) fn is_stored(&self) -> bool {
        self.storage.is_some()
            || self.values.is_storage_backed()
            || self.lengths.is_storage_backed()
            || self
                .structural
                .as_ref()
                .is_some_and(|facts| facts.postings.is_storage_backed())
    }

    fn materialize_storage(&mut self) -> Result<(), String> {
        if self.storage.is_none() {
            return Ok(());
        }
        self.ensure_storage_loaded()?;
        let mut loaded = self
            .loaded
            .get()
            .expect("stored posting index was loaded")
            .as_ref()
            .clone();
        loaded.storage = None;
        loaded.loaded = Arc::new(OnceLock::new());
        loaded.load_lock = Arc::new(std::sync::Mutex::new(()));
        *self = loaded;
        Ok(())
    }

    fn payload(&self) -> &Self {
        self.loaded
            .get()
            .map(|loaded| loaded.as_ref())
            .unwrap_or(self)
    }

    pub(crate) fn materialized(&self) -> &Self {
        self.payload()
    }

    fn configuration_matches(&self, field: &FieldDefinition, schema: &Schema) -> bool {
        let auto_prohibited = !field.indexed && schema.is_auto_index_prohibited(&field.path);
        let auto_length = matches!(field.kind, LogicalType::Array(_)) && !auto_prohibited;
        let auto_empty = field.kind == LogicalType::String && !field.multi && !auto_prohibited;
        self.path == field.path
            && self.kind == field.kind
            && self.multi == field.multi
            && self.values_indexed == field.indexed
            && self.value_complete == field.indexed
            && self.length_complete == auto_length
            && self.empty_string_complete == auto_empty
            && self.has_structural
    }

    /// Borrow the ordered value keys of this posting index. Completed indexes
    /// traverse their archived B-tree or archived reverse directory.
    pub(crate) fn value_range(
        &self,
        lower: std::ops::Bound<ScalarKey>,
        upper: std::ops::Bound<ScalarKey>,
        descending: bool,
    ) -> PostingValueCursor<'_> {
        self.payload().values.range(lower, upper, descending)
    }

    pub(crate) fn value_cardinality(&self, key: &ScalarKey) -> u64 {
        self.payload().values.cardinality(key)
    }

    pub(crate) fn value_posting(&self, key: &ScalarKey) -> Option<&RoaringBitmap> {
        self.payload().values.get(key)
    }

    /// A broad direct relation can match thousands of target IDs. Scan each
    /// ordered source dictionary once when that costs less than thousands of
    /// independent seeks and repeated key decoding.
    pub(crate) fn union_id_postings(&self, ids: &HashSet<String>) -> (RoaringBitmap, u64, u64) {
        let this = self.payload();
        let mut union = RoaringBitmap::new();
        let mut lists = 0u64;
        let mut members = 0u64;
        if ids.len().saturating_mul(8) < this.values.len() {
            for id in ids {
                if let Some(slots) = this.values.get(&ScalarKey::Id(id.clone())) {
                    lists += 1;
                    members += slots.len();
                    union |= slots;
                }
            }
        } else {
            match &this.values.0 {
                PostingDictionaryState::Builder(builder) => {
                    for (key, ordinal) in &builder.values {
                        if let StoredScalarKey::Id(id) = key
                            && ids.contains(id)
                        {
                            let slots = builder
                                .bitmaps
                                .get(*ordinal as usize)
                                .expect("posting bitmap slot");
                            lists += 1;
                            members += slots.len();
                            union |= slots;
                        }
                    }
                }
                PostingDictionaryState::Published(values) => match &values.backing {
                    PublishedPostingBacking::Native(map) => {
                        for (key, ordinal) in map.iter() {
                            let StoredScalarKey::Id(id) = key else {
                                continue;
                            };
                            if ids.contains(id) {
                                let slots = values
                                    .bitmaps
                                    .get(*ordinal as usize)
                                    .expect("validated posting slot");
                                lists += 1;
                                members += slots.len();
                                union |= slots;
                            }
                        }
                    }
                    PublishedPostingBacking::Archived(archive) => {
                        for entry in archive.root().ascending.iter() {
                            let rkyv::Archived::<StoredScalarKey>::Id(id) = &entry.key else {
                                continue;
                            };
                            if ids.contains(id.as_str()) {
                                let slots = values
                                    .bitmaps
                                    .get(entry.ordinal.to_native() as usize)
                                    .expect("validated posting ordinal");
                                lists += 1;
                                members += slots.len();
                                union |= slots;
                            }
                        }
                    }
                    PublishedPostingBacking::Stored(stored) => {
                        for (key, ordinal) in stored.values.iter() {
                            let StoredScalarKey::Id(id) = key else {
                                continue;
                            };
                            if ids.contains(id) {
                                let slots = stored
                                    .load_ordinal(*ordinal as usize)
                                    .expect("stored posting leaf");
                                lists += 1;
                                members += slots.len();
                                union |= slots;
                            }
                        }
                    }
                },
            }
        }
        (union, lists, members)
    }

    pub(crate) fn value_posting_restricted(
        &self,
        key: &ScalarKey,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        self.payload().values.get(key).map(|bitmap| input & bitmap)
    }

    pub(crate) fn for_each_value(
        &self,
        visit: impl FnMut(&ScalarKey, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        self.payload().values.for_each(visit)
    }

    pub(crate) fn for_each_length(
        &self,
        visit: impl FnMut(&Number, &RoaringBitmap) -> Result<(), String>,
    ) -> Result<(), String> {
        self.payload().lengths.for_each(visit)
    }

    pub(crate) fn union_scalar_range(
        &self,
        key: &ScalarKey,
        operator: Operator,
        input: Option<&RoaringBitmap>,
    ) -> RoaringBitmap {
        scalar_range_postings(&self.payload().values, key, operator, input).0
    }

    fn state_source(&self, derived: Derived) -> Option<Option<&RoaringBitmap>> {
        let this = self.payload();
        let key = match derived {
            Derived::Exists => return None,
            Derived::Null if !this.multi => return Some(Some(&this.nulls)),
            Derived::Null => StructuralKey::AnyNull,
            Derived::IsNonEmpty => StructuralKey::NonEmpty,
            Derived::IsString => StructuralKey::String,
            Derived::IsNumber => StructuralKey::Number,
            Derived::IsArray => StructuralKey::Array,
            Derived::IsObject => StructuralKey::Object,
            Derived::None | Derived::Length => return None,
        };
        Some(this.structural.as_ref()?.postings.get(&key))
    }

    fn state_truth(term: &BoundTerm) -> Option<bool> {
        let Value::Bool(literal) = term.literal else {
            return None;
        };
        match term.operator {
            Operator::Eq => Some(literal),
            Operator::Ne => Some(!literal),
            _ => None,
        }
    }

    /// Inferred dictionary costing without materializing or unioning postings.
    pub(crate) fn estimate_observed_scalar(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain_len: u64,
        visits: &mut u64,
    ) -> Option<(f64, u64)> {
        let this = self.payload();
        if self.covered_records as u64 != domain_len {
            return None;
        }
        let equal = || this.values.cardinality(key);
        match operator {
            Operator::Eq => {
                *visits += 1;
                Some((1.0, equal()))
            }
            Operator::Ne if !self.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count - self.stats.null_count - equal(),
                ))
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                let keys = scalar_range_key_probe(&this.values, key, operator, 33);
                *visits += keys as u64;
                let count = if keys <= 32 {
                    keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if keys <= 32 {
                    keys as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, count))
            }
            _ => None,
        }
    }

    /// An observed scalar field can prove a dynamic comparison even when the
    /// schema-free binder conservatively marks its term as multivalued.
    pub(crate) fn exact_observed_scalar_restricted(
        &self,
        key: &ScalarKey,
        operator: Operator,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        let this = self.payload();
        if !self.value_complete || self.multi || self.covered_records as u64 != domain.len() {
            return None;
        }
        match operator {
            Operator::Eq => Some(
                this.values
                    .get(key)
                    .map(|slots| input & slots)
                    .unwrap_or_default(),
            ),
            Operator::Ne => {
                let mut result = input & &this.presence;
                result -= &this.nulls;
                if let Some(equal) = this.values.get(key) {
                    result -= equal;
                }
                Some(result)
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                Some(scalar_range_postings(&this.values, key, operator, Some(input)).0)
            }
            _ => None,
        }
    }

    pub(crate) fn merge_shifted(&mut self, mut batch: Self, base: u32) -> Result<(), String> {
        self.materialize_storage()?;
        batch.materialize_storage()?;
        if self.path != batch.path
            || self.kind != batch.kind
            || self.multi != batch.multi
            || self.values_indexed != batch.values_indexed
            || self.value_complete != batch.value_complete
            || self.length_complete != batch.length_complete
            || self.empty_string_complete != batch.empty_string_complete
            || self.structural.is_some() != batch.structural.is_some()
        {
            return Err(format!(
                "posting batch representation mismatch: path={:?}/{:?} kind={:?}/{:?} multi={}/{} values={}/{} value_complete={}/{} length={}/{} empty={}/{} structural={}/{}",
                self.path,
                batch.path,
                self.kind,
                batch.kind,
                self.multi,
                batch.multi,
                self.values_indexed,
                batch.values_indexed,
                self.value_complete,
                batch.value_complete,
                self.length_complete,
                batch.length_complete,
                self.empty_string_complete,
                batch.empty_string_complete,
                self.structural.is_some(),
                batch.structural.is_some(),
            ));
        }
        let covered = self
            .covered_records
            .checked_add(batch.covered_records)
            .ok_or("posting coverage overflow")?;
        append_shifted_bitmap(&mut self.presence, &batch.presence, base)?;
        append_shifted_bitmap(&mut self.empty_selections, &batch.empty_selections, base)?;
        append_shifted_bitmap(&mut self.nulls, &batch.nulls, base)?;
        if let (Some(target), Some(source)) = (&mut self.structural, batch.structural) {
            target.merge_shifted(source, base)?;
        }
        let mut values = batch.values.into_native_builder()?;
        for (key, ordinal) in values.values {
            let posting = values
                .bitmaps
                .get_mut(ordinal as usize)
                .ok_or("posting bitmap slot out of bounds")?;
            append_shifted_bitmap(self.values.bitmap_mut(key)?, posting, base)?;
        }
        let mut lengths = batch.lengths.into_native_builder()?;
        for (key, ordinal) in lengths.values {
            let posting = lengths
                .bitmaps
                .get_mut(ordinal as usize)
                .ok_or("posting bitmap slot out of bounds")?;
            append_shifted_bitmap(self.lengths.bitmap_mut(key)?, posting, base)?;
        }
        self.covered_records = covered;
        Ok(())
    }

    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        if !self.presence.is_subset(allowed)
            || !self.empty_selections.is_subset(allowed)
            || !self.nulls.is_subset(allowed)
            || self.structural.as_ref().is_some_and(|facts| {
                let mut outside = false;
                facts
                    .postings
                    .for_each_bitmap(|slots| {
                        outside |= !slots.is_subset(allowed);
                        Ok(())
                    })
                    .expect("validated structural archive");
                outside
            })
            || {
                let mut outside = false;
                self.values
                    .for_each_bitmap(|slots| {
                        outside |= !slots.is_subset(allowed);
                        Ok(())
                    })
                    .expect("validated scalar archive");
                self.lengths
                    .for_each_bitmap(|slots| {
                        outside |= !slots.is_subset(allowed);
                        Ok(())
                    })
                    .expect("validated number archive");
                outside
            }
        {
            return Err("posting outside logical eligible domain".into());
        }
        Ok(())
    }

    /// Intersect borrowed broad postings with a restricted domain before any copy.
    pub fn exact_set_restricted(
        &self,
        term: &BoundTerm,
        domain: &RoaringBitmap,
        input: &RoaringBitmap,
    ) -> Option<RoaringBitmap> {
        let this = self.payload();
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            return this.temporal_postings(term, Some(input));
        }
        if term.derived == Derived::Exists {
            let truth = Self::state_truth(term)?;
            if this.empty_selections.is_empty() {
                return Some(if truth {
                    input & &this.presence
                } else {
                    input - &this.presence
                });
            }
            let mut present = input & &this.presence;
            present -= &this.empty_selections;
            return Some(if truth { present } else { input - &present });
        }
        if let Some(source) = this.state_source(term.derived) {
            return Some(match (Self::state_truth(term)?, source) {
                (true, Some(slots)) => input & slots,
                (false, Some(slots)) => input - slots,
                (true, None) => RoaringBitmap::new(),
                (false, None) => input.clone(),
            });
        }
        let borrowed = match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length if !self.length_complete || !length_posting_is_exact(term) => {
                return None;
            }
            Derived::Length if term.operator == Operator::Eq => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                return Some(
                    this.lengths
                        .get(number)
                        .map(|set| input & set)
                        .unwrap_or_default(),
                );
            }
            Derived::Length if is_range(term.operator) => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                let restriction = (input.len() < domain.len()).then_some(input);
                return Some(range_postings(&this.lengths, number, term.operator, restriction).0);
            }
            Derived::Length if term.operator == Operator::Ne => {
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                let mut result = RoaringBitmap::new();
                this.lengths
                    .for_each(|key, slots| {
                        if key != number {
                            result |= input & slots;
                        }
                        Ok(())
                    })
                    .expect("validated number archive");
                return Some(result);
            }
            Derived::None if matches!(term.literal, Value::Null) => {
                // The null posting tracks only an explicit null field. It cannot
                // prove any/all predicates over an array's elements.
                if term.multi {
                    return None;
                }
                return match term.operator {
                    Operator::Eq => Some(input & &this.nulls),
                    Operator::Ne => Some(&(input & &this.presence) - &this.nulls),
                    _ => None,
                };
            }
            Derived::None
                if term.transform == Transform::Identity
                    && (!term.multi || term.quantifier == Quantifier::Any)
                    && term.operator == Operator::Eq
                    && this.scalar_equality_complete(term) =>
            {
                let key = ScalarKey::from_value(&term.literal)?;
                this.values.get(&key)
            }
            Derived::None
                if term.transform == Transform::Identity
                    && !term.multi
                    && this.value_complete
                    && is_range(term.operator) =>
            {
                let key = ScalarKey::from_value(&term.literal)?;
                let restriction = (input.len() < domain.len()).then_some(input);
                return Some(
                    scalar_range_postings(&this.values, &key, term.operator, restriction).0,
                );
            }
            _ => None,
        };
        if let Some(set) = borrowed {
            return Some(input & set);
        }
        this.exact_set(term, domain).map(|set| &set & input)
    }

    pub fn new(field: &FieldDefinition) -> Self {
        Self {
            path: field.path.clone(),
            kind: field.kind.clone(),
            multi: field.multi,
            presence: RoaringBitmap::new(),
            empty_selections: RoaringBitmap::new(),
            nulls: RoaringBitmap::new(),
            structural: None,
            values: PostingDictionary::builder(),
            lengths: PostingDictionary::builder(),
            values_indexed: true,
            value_complete: true,
            length_complete: matches!(field.kind, LogicalType::Array(_)),
            empty_string_complete: field.kind == LogicalType::String && !field.multi,
            covered_records: 0,
            stats: PostingStats::default(),
            has_structural: false,
            storage: None,
            loaded: Arc::new(OnceLock::new()),
            load_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }

    fn structural(field: &FieldDefinition) -> Self {
        let mut index = Self::new(field);
        index.values_indexed = field.indexed;
        index.value_complete = field.indexed;
        index.structural = Some(StructuralFacts::default());
        index.has_structural = true;
        index
    }

    fn scalar_equality_complete(&self, term: &BoundTerm) -> bool {
        self.value_complete
            || (self.empty_string_complete
                && !term.multi
                && !term.query_path.has_explicit_array_step
                && self.kind == LogicalType::String
                && matches!(&term.literal, Value::String(value) if value.is_empty()))
    }

    pub fn finish_stats(&mut self) {
        self.stats = PostingStats {
            present_count: self.presence.len(),
            null_count: self.nulls.len(),
            distinct_count: self.values.len(),
            min_key: self.values.first_public(),
            max_key: self.values.last_public(),
        };
    }

    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        let selected = data_values_small(record, &self.path);
        self.add_values(slot, selected.as_deref())
    }

    pub fn add_values(
        &mut self,
        slot: RecordSlot,
        selected: Option<&[&Value]>,
    ) -> Result<(), String> {
        self.covered_records = self
            .covered_records
            .checked_add(1)
            .ok_or("posting coverage overflow")?;
        let Some(selected) = selected else {
            return Ok(());
        };
        self.presence.insert(slot.0);
        if selected.is_empty() {
            self.empty_selections.insert(slot.0);
            return Ok(());
        }
        if let Some(facts) = &mut self.structural {
            for value in selected {
                facts.observe(slot, value, self.multi);
            }
        }
        if selected.len() == 1 && matches!(selected[0], Value::Null) {
            self.nulls.insert(slot.0);
            return Ok(());
        }
        if let [value] = selected {
            if !matches!(value, Value::Array(_)) {
                if let Value::String(text) = value
                    && text.is_empty()
                    && self.empty_string_complete
                    && !self.value_complete
                {
                    self.values
                        .bitmap_mut(StoredScalarKey::String(String::new()))?
                        .insert(slot.0);
                } else if self.value_complete
                    && let Some(key) = StoredScalarKey::from_value(value)
                {
                    self.values.bitmap_mut(key)?.insert(slot.0);
                }
                return Ok(());
            }
        }
        if !self.value_complete {
            if self.length_complete {
                for value in selected {
                    if let Value::Array(array) = value {
                        self.lengths
                            .bitmap_mut(PostingNumberKey::from_number(&Number::integer(
                                array.values.len() as i128,
                            )))?
                            .insert(slot.0);
                    }
                }
            }
            return Ok(());
        }
        let mut unique = std::collections::BTreeSet::new();
        for value in selected {
            if let Value::Array(array) = value {
                if self.length_complete {
                    self.lengths
                        .bitmap_mut(PostingNumberKey::from_number(&Number::integer(
                            array.values.len() as i128,
                        )))?
                        .insert(slot.0);
                }
                for item in &array.values {
                    if let Some(key) = StoredScalarKey::from_value(item) {
                        unique.insert(key);
                    }
                }
            } else if let Some(key) = StoredScalarKey::from_value(value) {
                unique.insert(key);
            }
        }
        for key in unique {
            self.values.bitmap_mut(key)?.insert(slot.0);
        }
        Ok(())
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        self.validate_eligible(record_count, record_count)
    }

    /// Check complete logical coverage while retaining the full slot bound.
    pub fn validate_eligible(
        &self,
        slot_count: usize,
        eligible_count: usize,
    ) -> Result<(), String> {
        self.validate_completed(slot_count, eligible_count, None)
    }

    /// The one completed-component validation pass. When an eligible domain is
    /// narrower than the physical slot domain, it is checked in the same
    /// bitmap traversal instead of scanning every dictionary a second time.
    pub(crate) fn validate_completed(
        &self,
        slot_count: usize,
        eligible_count: usize,
        allowed: Option<&RoaringBitmap>,
    ) -> Result<(), String> {
        self.ensure_storage_loaded()?;
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.posting_logical_validation_passes =
                counters.posting_logical_validation_passes.saturating_add(1);
        });
        if !self.values_indexed && self.value_complete {
            return Err("structural-only posting has complete values".into());
        }
        if self.covered_records as usize != eligible_count {
            return Err("incomplete posting coverage".into());
        }
        let valid = |set: &RoaringBitmap| {
            set.max().is_none_or(|s| (s as usize) < slot_count)
                && allowed.is_none_or(|allowed| set.is_subset(allowed))
        };
        let mut visited = 3u64;
        let mut invalid_domain =
            !valid(&self.presence) || !valid(&self.empty_selections) || !valid(&self.nulls);
        let mut inconsistent_structure = false;
        if let Some(facts) = &self.structural {
            facts
                .postings
                .for_each_bitmap(|slots| {
                    visited = visited.saturating_add(1);
                    invalid_domain |= !valid(slots);
                    inconsistent_structure |= !slots.is_subset(&self.presence);
                    Ok(())
                })
                .expect("validated structural archive");
        }
        self.values
            .for_each_bitmap(|slots| {
                visited = visited.saturating_add(1);
                invalid_domain |= !valid(slots);
                Ok(())
            })
            .expect("validated scalar archive");
        self.lengths
            .for_each_bitmap(|slots| {
                visited = visited.saturating_add(1);
                invalid_domain |= !valid(slots);
                Ok(())
            })
            .expect("validated number archive");
        if invalid_domain {
            return Err("posting contains out-of-domain record slot".into());
        }
        if !self.nulls.is_subset(&self.presence) {
            return Err("null posting is not present".into());
        }
        if !self.empty_selections.is_subset(&self.presence) {
            return Err("empty selection is not present".into());
        }
        if self.structural.as_ref().is_some_and(|facts| {
            inconsistent_structure
                || facts
                    .postings
                    .get(&StructuralKey::EmptyObject)
                    .is_some_and(|empty| {
                        !empty.is_subset(
                            facts
                                .postings
                                .get(&StructuralKey::Object)
                                .unwrap_or(&RoaringBitmap::new()),
                        )
                    })
        }) {
            return Err("structural posting is inconsistent with presence or type".into());
        }
        if self.stats.present_count != self.presence.len()
            || self.stats.null_count != self.nulls.len()
            || self.stats.distinct_count != self.distinct_values()
        {
            return Err("posting statistics mismatch".into());
        }
        if !self.length_complete && !self.lengths.is_empty() {
            return Err("posting capability and stored facts mismatch".into());
        }
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.posting_logical_visits =
                counters.posting_logical_visits.saturating_add(visited);
        });
        Ok(())
    }

    pub fn distinct_values(&self) -> usize {
        self.values.len()
    }

    pub fn value_membership_count(&self) -> u64 {
        let mut total = 0u64;
        self.values
            .for_each(|_, bitmap| {
                total = total.saturating_add(bitmap.len());
                Ok(())
            })
            .expect("validated scalar posting archive");
        total
    }

    /// Native Roaring payload bytes plus the checked key archive bytes.
    pub fn dictionary_payload_bytes(&self) -> usize {
        let mut total = self.archive_bytes();
        self.values
            .for_each_bitmap(|bitmap| {
                total = total.saturating_add(bitmap.serialized_size());
                Ok(())
            })
            .expect("validated scalar posting archive");
        self.lengths
            .for_each_bitmap(|bitmap| {
                total = total.saturating_add(bitmap.serialized_size());
                Ok(())
            })
            .expect("validated number posting archive");
        if let Some(facts) = &self.structural {
            facts
                .postings
                .for_each_bitmap(|bitmap| {
                    total = total.saturating_add(bitmap.serialized_size());
                    Ok(())
                })
                .expect("validated structural posting archive");
        }
        total
    }

    pub(crate) fn dictionary_bitmap_payload_bytes(&self) -> usize {
        let mut total = 0usize;
        self.values
            .for_each_bitmap(|bitmap| {
                total = total.saturating_add(bitmap.serialized_size());
                Ok(())
            })
            .expect("validated scalar posting archive");
        self.lengths
            .for_each_bitmap(|bitmap| {
                total = total.saturating_add(bitmap.serialized_size());
                Ok(())
            })
            .expect("validated number posting archive");
        if let Some(facts) = &self.structural {
            facts
                .postings
                .for_each_bitmap(|bitmap| {
                    total = total.saturating_add(bitmap.serialized_size());
                    Ok(())
                })
                .expect("validated structural posting archive");
        }
        total
    }

    pub(crate) fn native_bitmap_payload_bytes(&self) -> usize {
        self.presence.serialized_size()
            + self.empty_selections.serialized_size()
            + self.nulls.serialized_size()
            + self.dictionary_bitmap_payload_bytes()
    }

    /// Freeze every ordered posting dictionary into checked immutable storage.
    ///
    /// This boundary is called from segment finalization, after all inference
    /// admission and semantic posting validation has completed.
    pub(crate) fn seal_native_storage(&mut self) -> Result<(), String> {
        self.values =
            std::mem::replace(&mut self.values, PostingDictionary::builder()).publish_native()?;
        self.lengths =
            std::mem::replace(&mut self.lengths, PostingDictionary::builder()).publish_native()?;
        if let Some(facts) = &mut self.structural {
            facts.postings = std::mem::replace(&mut facts.postings, PostingDictionary::builder())
                .publish_native()?;
        }
        Ok(())
    }

    pub(crate) fn thaw_storage(&mut self) -> Result<(), String> {
        let values = std::mem::replace(&mut self.values, PostingDictionary::builder())
            .into_native_builder()?;
        self.values = PostingDictionary::from_native_builder(values);
        let lengths = std::mem::replace(&mut self.lengths, PostingDictionary::builder())
            .into_native_builder()?;
        self.lengths = PostingDictionary::from_native_builder(lengths);
        if let Some(facts) = &mut self.structural {
            let postings = std::mem::replace(&mut facts.postings, PostingDictionary::builder())
                .into_native_builder()?;
            facts.postings = PostingDictionary::from_native_builder(postings);
        }
        Ok(())
    }

    pub(crate) fn thaw_values_storage(&mut self) -> Result<(), String> {
        let values = std::mem::replace(&mut self.values, PostingDictionary::builder())
            .into_native_builder()?;
        self.values = PostingDictionary::from_native_builder(values);
        Ok(())
    }

    pub(crate) fn is_archived(&self) -> bool {
        self.storage.is_some()
            || self.values.is_archived()
                && self.lengths.is_archived()
                && self
                    .structural
                    .as_ref()
                    .is_none_or(|facts| facts.postings.is_archived())
    }

    #[allow(dead_code)]
    pub(crate) fn is_native(&self) -> bool {
        if self.storage.is_some() {
            return false;
        }
        self.values.is_native()
            && self.lengths.is_native()
            && self
                .structural
                .as_ref()
                .is_none_or(|facts| facts.postings.is_native())
    }

    pub(crate) fn is_published(&self) -> bool {
        self.values.is_published()
            && self.lengths.is_published()
            && self
                .structural
                .as_ref()
                .is_none_or(|facts| facts.postings.is_published())
    }

    pub(crate) fn storage_representation_counts(&self) -> (usize, usize) {
        if self.storage.is_some() {
            return (0, 3);
        }
        let mut native = 0usize;
        let mut archived = 0usize;
        if self.values.is_archived() {
            archived += 1;
        } else {
            native += 1;
        }
        if self.lengths.is_archived() {
            archived += 1;
        } else {
            native += 1;
        }
        if let Some(facts) = &self.structural {
            if facts.postings.is_archived() {
                archived += 1;
            } else {
                native += 1;
            }
        }
        (native, archived)
    }

    #[cfg(test)]
    pub(crate) fn value_bitmap_owner_id(&self) -> usize {
        self.values.bitmap_owner_id()
    }

    pub(crate) fn archived(&self) -> Result<Self, String> {
        let mut archived = self.clone();
        archived.archive_storage()?;
        Ok(archived)
    }

    pub(crate) fn archive_storage(&mut self) -> Result<(), String> {
        if self.storage.is_some() {
            return Ok(());
        }
        let values = self.values.archived()?;
        let lengths = self.lengths.archived()?;
        let structural = self
            .structural
            .as_ref()
            .map(|facts| facts.postings.archived())
            .transpose()?;
        self.values = values;
        self.lengths = lengths;
        if let (Some(facts), Some(structural)) = (&mut self.structural, structural) {
            facts.postings = structural;
        }
        Ok(())
    }

    pub(crate) fn archive_bytes(&self) -> usize {
        if self.storage.is_some() {
            return 0;
        }
        self.values
            .archive_bytes()
            .saturating_add(self.lengths.archive_bytes())
            .saturating_add(
                self.structural
                    .as_ref()
                    .map_or(0, |facts| facts.postings.archive_bytes()),
            )
    }

    pub(crate) fn for_each_archive_owner(&self, visit: &mut impl FnMut(usize, usize, usize)) {
        self.values.for_each_archive_owner(visit);
        self.lengths.for_each_archive_owner(visit);
        if let Some(facts) = &self.structural {
            facts.postings.for_each_archive_owner(visit);
        }
    }

    pub(crate) fn distinct_lengths(&self) -> usize {
        self.lengths.len()
    }

    /// Cheap cardinality/work estimate; no dictionary traversal or bitmap copy.
    pub fn estimate_exact(&self, term: &BoundTerm, domain_len: u64) -> Option<(f64, u64)> {
        self.estimate_exact_with_visits(term, domain_len, &mut 0)
    }

    pub(crate) fn estimate_exact_with_visits(
        &self,
        term: &BoundTerm,
        domain_len: u64,
        visits: &mut u64,
    ) -> Option<(f64, u64)> {
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain_len {
            return None;
        }
        if self.is_stored() && self.loaded.get().is_none() {
            *visits += 1;
            let estimate = domain_len / self.stats.distinct_count.max(1) as u64;
            return Some((1.0 + self.stats.distinct_count as f64 / 8.0, estimate));
        }
        let this = self.payload();
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            if term.multi || term.transform != Transform::Identity {
                return None;
            }
            let keys = this.temporal_key_probe(term, 33)?;
            *visits += keys as u64;
            let count = if keys <= 32 {
                keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
            } else {
                domain_len / 4
            };
            return Some((1.0 + keys as f64, count));
        }
        if term.derived == Derived::Exists {
            *visits += 1;
            let yes = this
                .presence
                .len()
                .saturating_sub(self.empty_selections.len());
            return Some((
                1.0,
                if Self::state_truth(term)? {
                    yes
                } else {
                    domain_len - yes
                },
            ));
        }
        if let Some(source) = this.state_source(term.derived) {
            *visits += 1;
            let yes = source.map_or(0, RoaringBitmap::len);
            return Some((
                1.0,
                if Self::state_truth(term)? {
                    yes
                } else {
                    domain_len - yes
                },
            ));
        }
        match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length => {
                if !self.length_complete || !length_posting_is_exact(term) {
                    return None;
                }
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                let range_keys = is_range(term.operator)
                    .then(|| range_key_probe(&this.lengths, number, term.operator, 33));
                *visits += range_keys.unwrap_or(1) as u64;
                let count = if term.operator == Operator::Eq {
                    this.lengths
                        .get(number)
                        .map(RoaringBitmap::len)
                        .unwrap_or(0)
                } else if let Some(keys) = range_keys {
                    if keys <= 32 {
                        keys as u64 * domain_len / this.lengths.len().max(1) as u64
                    } else {
                        domain_len / 4
                    }
                } else {
                    domain_len / 4
                };
                let work = if let Some(keys) = range_keys {
                    if keys <= 32 {
                        keys as f64
                    } else {
                        this.lengths.len() as f64
                    }
                } else if term.operator == Operator::Eq {
                    0.0
                } else {
                    this.lengths.len() as f64
                };
                return Some((1.0 + work, count));
            }
            Derived::None => {}
            Derived::IsNonEmpty
            | Derived::IsString
            | Derived::IsNumber
            | Derived::IsArray
            | Derived::IsObject => return None,
        }
        if matches!(term.literal, Value::Null) {
            if term.multi {
                return None;
            }
            return match term.operator {
                Operator::Eq => Some((1.0, self.stats.null_count)),
                Operator::Ne => Some((2.0, self.stats.present_count - self.stats.null_count)),
                _ => None,
            };
        }
        if term.transform != Transform::Identity
            || (term.multi && (term.quantifier != Quantifier::Any || term.operator != Operator::Eq))
        {
            return None;
        }
        let key = ScalarKey::from_value(&term.literal)?;
        if term.operator == Operator::Eq && !this.scalar_equality_complete(term) {
            return None;
        }
        if term.operator != Operator::Eq && !self.value_complete {
            return None;
        }
        match term.operator {
            Operator::Eq => {
                *visits += 1;
                Some((1.0, this.values.cardinality(&key)))
            }
            Operator::Ne if !term.multi => {
                *visits += 1;
                Some((
                    2.0,
                    self.stats.present_count
                        - self.stats.null_count
                        - this.values.cardinality(&key),
                ))
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                let keys = scalar_range_key_probe(&this.values, &key, term.operator, 33);
                *visits += keys as u64;
                let estimate = if keys <= 32 {
                    keys as u64 * self.stats.present_count / self.stats.distinct_count.max(1) as u64
                } else {
                    domain_len / 4
                };
                let work = if keys <= 32 {
                    keys as f64
                } else {
                    self.stats.distinct_count as f64 / 8.0
                };
                Some((1.0 + work, estimate))
            }
            _ => None,
        }
    }

    /// An exact answer over this index's complete record domain, or `None`
    /// when quantification/transformation needs independent verification.
    pub fn exact_set(&self, term: &BoundTerm, domain: &RoaringBitmap) -> Option<RoaringBitmap> {
        self.ensure_storage_loaded().ok()?;
        let this = self.payload();
        if term
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.posting_compatible())
        {
            return None;
        }
        if self.covered_records as u64 != domain.len() {
            return None;
        }
        if term.period.is_some() {
            if !self.value_complete {
                return None;
            }
            return this.temporal_postings(term, None);
        }
        if term.derived == Derived::Exists {
            let truth = Self::state_truth(term)?;
            if this.empty_selections.is_empty() {
                return Some(if truth {
                    this.presence.clone()
                } else {
                    domain - &this.presence
                });
            }
            let mut present = this.presence.clone();
            present -= &this.empty_selections;
            return Some(if truth { present } else { domain - &present });
        }
        if let Some(source) = this.state_source(term.derived) {
            return Some(match (Self::state_truth(term)?, source) {
                (true, Some(slots)) => slots.clone(),
                (false, Some(slots)) => domain - slots,
                (true, None) => RoaringBitmap::new(),
                (false, None) => domain.clone(),
            });
        }
        match term.derived {
            Derived::Exists | Derived::Null => return None,
            Derived::Length => {
                if !self.length_complete || !length_posting_is_exact(term) {
                    return None;
                }
                let Value::Number(number) = &term.literal else {
                    return None;
                };
                return match term.operator {
                    Operator::Eq => Some(this.lengths.get(number).cloned().unwrap_or_default()),
                    Operator::Ne => {
                        let mut result = RoaringBitmap::new();
                        this.lengths
                            .for_each(|key, slots| {
                                if key != number {
                                    result |= slots;
                                }
                                Ok(())
                            })
                            .expect("validated number archive");
                        Some(result)
                    }
                    op if is_range(op) => Some(range_postings(&this.lengths, &number, op, None).0),
                    _ => None,
                };
            }
            Derived::None => {}
            Derived::IsNonEmpty
            | Derived::IsString
            | Derived::IsNumber
            | Derived::IsArray
            | Derived::IsObject => return None,
        }
        if matches!(term.literal, Value::Null) {
            if term.multi {
                return None;
            }
            return Some(match term.operator {
                Operator::Eq => this.nulls.clone(),
                Operator::Ne => &this.presence - &this.nulls,
                _ => return None,
            });
        }
        if term.transform != Transform::Identity {
            return None;
        }
        let key = ScalarKey::from_value(&term.literal)?;
        if term.operator == Operator::Eq && !this.scalar_equality_complete(term) {
            return None;
        }
        if term.operator != Operator::Eq && !self.value_complete {
            return None;
        }
        if term.multi && term.quantifier != Quantifier::Any {
            return None;
        }
        if term.multi && term.operator != Operator::Eq {
            return None;
        }
        match term.operator {
            Operator::Eq => Some(this.values.get(&key).cloned().unwrap_or_default()),
            Operator::Ne if !term.multi => Some(
                &(&this.presence - &this.nulls)
                    - &this.values.get(&key).cloned().unwrap_or_default(),
            ),
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le if !term.multi => {
                Some(scalar_range_postings(&this.values, &key, term.operator, None).0)
            }
            _ => None,
        }
    }

    fn temporal_postings(
        &self,
        term: &BoundTerm,
        input: Option<&RoaringBitmap>,
    ) -> Option<RoaringBitmap> {
        let this = self.payload();
        if term.multi || term.transform != Transform::Identity || term.derived != Derived::None {
            return None;
        }
        let period = term.period.as_ref()?;
        let start = ScalarKey::from_value(&period.start)?;
        let end = ScalarKey::from_value(&period.end)?;
        let type_tag = std::mem::discriminant(&start);
        if std::mem::discriminant(&end) != type_tag || start >= end {
            return None;
        }
        let same_type = |key: &ScalarKey| std::mem::discriminant(key) == type_tag;
        let ascending = |lower: std::ops::Bound<ScalarKey>, upper: std::ops::Bound<ScalarKey>| {
            this.values.range(lower, upper, false)
        };
        let descending = |lower: std::ops::Bound<ScalarKey>, upper: std::ops::Bound<ScalarKey>| {
            this.values.range(lower, upper, true)
        };
        Some(match term.operator {
            Operator::Eq => {
                let mut range = ascending(Included(start.clone()), Excluded(end.clone()));
                union_postings(std::iter::from_fn(move || range.next()), input).0
            }
            Operator::Ne => {
                let mut below = descending(Unbounded, Excluded(start.clone()));
                let mut result = union_postings(
                    std::iter::from_fn(move || {
                        let (key, posting) = below.next()?;
                        same_type(&key).then_some((key, posting))
                    }),
                    input,
                )
                .0;
                let mut above = ascending(Included(end.clone()), Unbounded);
                result |= union_postings(
                    std::iter::from_fn(move || {
                        let (key, posting) = above.next()?;
                        same_type(&key).then_some((key, posting))
                    }),
                    input,
                )
                .0;
                result
            }
            Operator::Lt | Operator::Le | Operator::Gt | Operator::Ge => {
                let (bound, descending_scan) = match term.operator {
                    Operator::Lt => (start.clone(), true),
                    Operator::Le => (end.clone(), true),
                    Operator::Gt => (end.clone(), false),
                    Operator::Ge => (start.clone(), false),
                    _ => unreachable!(),
                };
                let mut range = if descending_scan {
                    descending(Unbounded, Excluded(bound))
                } else {
                    ascending(Included(bound), Unbounded)
                };
                union_postings(
                    std::iter::from_fn(move || {
                        let (key, posting) = range.next()?;
                        same_type(&key).then_some((key, posting))
                    }),
                    input,
                )
                .0
            }
            _ => return None,
        })
    }

    fn temporal_key_probe(&self, term: &BoundTerm, limit: usize) -> Option<usize> {
        let period = term.period.as_ref()?;
        let start = ScalarKey::from_value(&period.start)?;
        let end = ScalarKey::from_value(&period.end)?;
        let type_tag = std::mem::discriminant(&start);
        if std::mem::discriminant(&end) != type_tag || start >= end {
            return None;
        }
        let same_type = |key: &ScalarKey| std::mem::discriminant(key) == type_tag;
        Some(match term.operator {
            Operator::Eq => {
                let mut range =
                    self.values
                        .range(Included(start.clone()), Excluded(end.clone()), false);
                std::iter::from_fn(move || range.next()).take(limit).count()
            }
            Operator::Ne => {
                let mut below = self.values.range(Unbounded, Excluded(start.clone()), true);
                let below = std::iter::from_fn(|| {
                    let (key, posting) = below.next()?;
                    same_type(&key).then_some((key, posting))
                })
                .take(limit)
                .count();
                let mut above = self.values.range(Included(end.clone()), Unbounded, false);
                below
                    + std::iter::from_fn(|| {
                        let (key, posting) = above.next()?;
                        same_type(&key).then_some((key, posting))
                    })
                    .take(limit.saturating_sub(below))
                    .count()
            }
            Operator::Lt | Operator::Le | Operator::Gt | Operator::Ge => {
                let (bound, descending_scan) = match term.operator {
                    Operator::Lt => (start.clone(), true),
                    Operator::Le => (end.clone(), true),
                    Operator::Gt => (end.clone(), false),
                    Operator::Ge => (start.clone(), false),
                    _ => unreachable!(),
                };
                let mut range = if descending_scan {
                    self.values.range(Unbounded, Excluded(bound), true)
                } else {
                    self.values.range(Included(bound), Unbounded, false)
                };
                std::iter::from_fn(|| {
                    let (key, posting) = range.next()?;
                    same_type(&key).then_some((key, posting))
                })
                .take(limit)
                .count()
            }
            _ => return None,
        })
    }
}

/// A length posting is a union of records containing at least one reached
/// array of that length. It proves existential comparisons. A one-segment
/// field path selects at most one array from a record root, so its default
/// universal comparison is also exact. Nested universal comparisons require
/// a scanner because the posting does not retain every reached length.
fn length_posting_is_exact(term: &BoundTerm) -> bool {
    !term.query_path.has_explicit_array_step
        && (term.quantifier == Quantifier::Any || term.path.segments.len() == 1)
}

fn derived_structural_key(derived: Derived) -> Option<StructuralKey> {
    match derived {
        Derived::Null => Some(StructuralKey::AnyNull),
        Derived::IsNonEmpty => Some(StructuralKey::NonEmpty),
        Derived::IsString => Some(StructuralKey::String),
        Derived::IsNumber => Some(StructuralKey::Number),
        Derived::IsArray => Some(StructuralKey::Array),
        Derived::IsObject => Some(StructuralKey::Object),
        _ => None,
    }
}

fn range_bounds<K: Clone + Ord>(
    key: &K,
    operator: Operator,
) -> (std::ops::Bound<K>, std::ops::Bound<K>) {
    match operator {
        Operator::Gt => (Excluded(key.clone()), Unbounded),
        Operator::Ge => (Included(key.clone()), Unbounded),
        Operator::Lt => (Unbounded, Excluded(key.clone())),
        Operator::Le => (Unbounded, Included(key.clone())),
        _ => (Unbounded, Unbounded),
    }
}

fn is_range(operator: Operator) -> bool {
    matches!(
        operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    )
}

fn union_postings<'a, K: 'a>(
    entries: impl Iterator<Item = (K, &'a RoaringBitmap)>,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize) {
    let mut result = RoaringBitmap::new();
    let mut visited = 0;
    for (_, posting) in entries {
        visited += 1;
        if let Some(input) = restriction {
            result |= input & posting;
        } else {
            result |= posting;
        }
    }
    (result, visited)
}

fn range_postings<K>(
    entries: &PostingDictionary<K>,
    key: &K::Query,
    operator: Operator,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize)
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
    K::Query: Clone,
{
    let mut range = match operator {
        Operator::Gt => entries.range(Excluded(key.clone()), Unbounded, false),
        Operator::Ge => entries.range(Included(key.clone()), Unbounded, false),
        Operator::Lt => entries.range(Unbounded, Excluded(key.clone()), false),
        Operator::Le => entries.range(Unbounded, Included(key.clone()), false),
        _ => return (RoaringBitmap::new(), 0),
    };
    union_postings(std::iter::from_fn(move || range.next()), restriction)
}

fn range_key_probe<K>(
    entries: &PostingDictionary<K>,
    key: &K::Query,
    operator: Operator,
    limit: usize,
) -> usize
where
    K: PostingDictionaryKey,
    K::Archived: Ord
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>>,
    K: for<'a> ArchiveSerialize<
        rkyv::api::high::HighSerializer<
            rkyv::util::AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
    K::Query: Clone,
{
    let (lower, upper) = match operator {
        Operator::Gt => (Excluded(key.clone()), Unbounded),
        Operator::Ge => (Included(key.clone()), Unbounded),
        Operator::Lt => (Unbounded, Excluded(key.clone())),
        Operator::Le => (Unbounded, Included(key.clone())),
        _ => return 0,
    };
    let mut range = entries.range(lower, upper, false);
    let mut count = 0;
    while count < limit && range.next().is_some() {
        count += 1;
    }
    count
}

fn scalar_range_key_probe(
    entries: &PostingDictionary<StoredScalarKey>,
    key: &ScalarKey,
    operator: Operator,
    limit: usize,
) -> usize {
    let same_type =
        |candidate: &ScalarKey| std::mem::discriminant(candidate) == std::mem::discriminant(key);
    let descending = matches!(operator, Operator::Lt | Operator::Le);
    let (lower, upper) = match operator {
        Operator::Gt => (Excluded(key.clone()), Unbounded),
        Operator::Ge => (Included(key.clone()), Unbounded),
        Operator::Lt => (Unbounded, Excluded(key.clone())),
        Operator::Le => (Unbounded, Included(key.clone())),
        _ => return 0,
    };
    let mut range = entries.range(lower, upper, descending);
    let mut count = 0;
    while count < limit {
        let Some((candidate, _)) = range.next() else {
            break;
        };
        if !same_type(&candidate) {
            break;
        }
        count += 1;
    }
    count
}

fn scalar_range_postings(
    entries: &PostingDictionary<StoredScalarKey>,
    key: &ScalarKey,
    operator: Operator,
    restriction: Option<&RoaringBitmap>,
) -> (RoaringBitmap, usize) {
    let same_type =
        |candidate: &ScalarKey| std::mem::discriminant(candidate) == std::mem::discriminant(key);
    let descending = matches!(operator, Operator::Lt | Operator::Le);
    let (lower, upper) = match operator {
        Operator::Gt => (Excluded(key.clone()), Unbounded),
        Operator::Ge => (Included(key.clone()), Unbounded),
        Operator::Lt => (Unbounded, Excluded(key.clone())),
        Operator::Le => (Unbounded, Included(key.clone())),
        _ => return (RoaringBitmap::new(), 0),
    };
    let mut range = entries.range(lower, upper, descending);
    union_postings(
        std::iter::from_fn(move || {
            let (candidate, posting) = range.next()?;
            same_type(&candidate).then_some((candidate, posting))
        }),
        restriction,
    )
}

/// Posting indexes registered for one collection.
#[derive(Clone, Debug)]
pub struct PostingCollection {
    pub fields: HashMap<Vec<String>, PostingIndex>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PostingCollectionSnapshot {
    #[serde(with = "crate::storage::serde_support::btree_map_sequence")]
    pub fields: BTreeMap<Vec<String>, PostingIndexSnapshot>,
}

impl PostingCollection {
    pub(crate) fn storage_snapshot(&self) -> Result<PostingCollectionSnapshot, String> {
        Ok(PostingCollectionSnapshot {
            fields: self
                .fields
                .iter()
                .map(|(path, index)| Ok((path.clone(), index.storage_snapshot()?)))
                .collect::<Result<_, String>>()?,
        })
    }

    pub(crate) fn configuration_matches(&self, schema: &Schema) -> bool {
        self.fields.len() == schema.fields.len()
            && self.fields.iter().all(|(path, index)| {
                schema
                    .fields
                    .get(path)
                    .is_some_and(|field| index.configuration_matches(field, schema))
            })
    }

    pub(crate) fn merge_shifted(&mut self, batch: Self, base: u32) -> Result<(), String> {
        if self.fields.len() != batch.fields.len() {
            return Err("posting batch field coverage mismatch".into());
        }
        for (path, index) in batch.fields {
            self.fields
                .get_mut(&path)
                .ok_or("posting batch field missing")?
                .merge_shifted(index, base)?;
        }
        Ok(())
    }

    pub(crate) fn merge_shifted_in_pool(
        &mut self,
        batch: Self,
        base: u32,
        _pool: &rayon::ThreadPool,
    ) -> Result<(), String> {
        if self.fields.len() != batch.fields.len() {
            return Err("posting batch field coverage mismatch".into());
        }
        for (path, index) in batch.fields {
            self.fields
                .get_mut(&path)
                .ok_or("posting batch field missing")?
                .merge_shifted(index, base)?;
        }
        Ok(())
    }

    pub fn validate_domain(&self, allowed: &RoaringBitmap) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate_domain(allowed)?;
        }
        Ok(())
    }

    pub fn new(schema: &Schema) -> Self {
        let fields = schema
            .fields
            .values()
            .map(|field| {
                let mut index = PostingIndex::structural(field);
                if !field.indexed && schema.is_auto_index_prohibited(&field.path) {
                    index.length_complete = false;
                    index.empty_string_complete = false;
                }
                (field.path.clone(), index)
            })
            .collect();
        Self { fields }
    }

    pub(crate) fn seal_native_storage(&mut self) -> Result<(), String> {
        for index in self.fields.values_mut() {
            index.seal_native_storage()?;
        }
        Ok(())
    }

    pub(crate) fn thaw_storage(&mut self) -> Result<(), String> {
        for index in self.fields.values_mut() {
            index.thaw_storage()?;
        }
        Ok(())
    }

    pub(crate) fn all_archived(&self) -> bool {
        self.fields.values().all(PostingIndex::is_archived)
    }

    #[allow(dead_code)]
    pub(crate) fn all_native(&self) -> bool {
        self.fields.values().all(PostingIndex::is_native)
    }

    pub(crate) fn all_published(&self) -> bool {
        self.fields.values().all(PostingIndex::is_published)
    }

    pub(crate) fn storage_representation_counts(&self) -> (usize, usize) {
        self.fields
            .values()
            .map(PostingIndex::storage_representation_counts)
            .fold((0, 0), |(native, archived), (add_native, add_archived)| {
                (native + add_native, archived + add_archived)
            })
    }

    pub(crate) fn archive_storage(&mut self) -> Result<(), String> {
        self.fields
            .values_mut()
            .try_for_each(PostingIndex::archive_storage)
    }

    pub(crate) fn archived(&self) -> Result<Self, String> {
        let fields = self
            .fields
            .iter()
            .map(|(path, index)| Ok((path.clone(), index.archived()?)))
            .collect::<Result<HashMap<_, _>, String>>()?;
        Ok(Self { fields })
    }

    pub(crate) fn for_each_archive_owner(&self, visit: &mut impl FnMut(usize, usize, usize)) {
        for index in self.fields.values() {
            index.for_each_archive_owner(visit);
        }
    }

    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        for index in self.fields.values_mut() {
            index.add(slot, record)?;
        }
        Ok(())
    }

    pub fn finish_stats(&mut self) {
        for index in self.fields.values_mut() {
            index.finish_stats();
        }
    }

    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        self.validate_eligible(record_count, record_count)
    }

    pub fn validate_eligible(
        &self,
        slot_count: usize,
        eligible_count: usize,
    ) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate_eligible(slot_count, eligible_count)?;
        }
        Ok(())
    }

    pub(crate) fn validate_completed(
        &self,
        slot_count: usize,
        eligible_count: usize,
        allowed: Option<&RoaringBitmap>,
    ) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate_completed(slot_count, eligible_count, allowed)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "posting.test.rs"]
mod tests;
