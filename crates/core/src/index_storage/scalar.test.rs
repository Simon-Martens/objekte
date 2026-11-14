use std::cmp::Ordering;

use time::{Date, Month, OffsetDateTime};

use crate::decimal::ExactDecimal;
use crate::model::Number;
use crate::posting::ScalarKey;

use super::scalar::{compare_scalar_keys, decode_scalar_key, encode_scalar_key};

fn keys() -> Vec<ScalarKey> {
    vec![
        ScalarKey::String(String::new()),
        ScalarKey::String("a".into()),
        ScalarKey::String("aa".into()),
        ScalarKey::String("b".into()),
        ScalarKey::String("ü".into()),
        ScalarKey::Id("a".into()),
        ScalarKey::Id("z".into()),
        ScalarKey::Number(Number::Integer(i128::MIN)),
        ScalarKey::Number(Number::Integer(-2)),
        ScalarKey::Number(Number::Integer(-1)),
        ScalarKey::Number(Number::Integer(0)),
        ScalarKey::Number(Number::Integer(1)),
        ScalarKey::Number(Number::Integer(2)),
        ScalarKey::Number(Number::Integer(i128::MAX)),
        ScalarKey::Number(Number::Float(-1.5)),
        ScalarKey::Number(Number::Float(-0.5)),
        ScalarKey::Number(Number::Float(0.0)),
        ScalarKey::Number(Number::Float(0.5)),
        ScalarKey::Number(Number::Float(1.5)),
        ScalarKey::Number(Number::ExactDecimal(ExactDecimal::parse("-1.25").unwrap())),
        ScalarKey::Number(Number::ExactDecimal(ExactDecimal::parse("0").unwrap())),
        ScalarKey::Number(Number::ExactDecimal(ExactDecimal::parse("1.25").unwrap())),
        ScalarKey::Bool(false),
        ScalarKey::Bool(true),
        ScalarKey::Date(Date::from_calendar_date(1969, Month::December, 31).unwrap()),
        ScalarKey::Date(Date::from_calendar_date(1970, Month::January, 1).unwrap()),
        ScalarKey::Date(Date::from_calendar_date(2026, Month::November, 14).unwrap()),
        ScalarKey::Datetime(OffsetDateTime::from_unix_timestamp_nanos(-1).unwrap()),
        ScalarKey::Datetime(OffsetDateTime::from_unix_timestamp_nanos(0).unwrap()),
        ScalarKey::Datetime(OffsetDateTime::from_unix_timestamp_nanos(1).unwrap()),
        ScalarKey::Unixtime(i64::MIN),
        ScalarKey::Unixtime(-1),
        ScalarKey::Unixtime(0),
        ScalarKey::Unixtime(i64::MAX),
    ]
}

#[test]
fn every_scalar_round_trips() {
    for key in keys() {
        let encoded = encode_scalar_key(&key).unwrap();
        let decoded = decode_scalar_key(&encoded).unwrap();
        assert_eq!(decoded, key);
    }
}

#[test]
fn encoded_comparison_matches_scalar_key_ordering() {
    let keys = keys();
    for left in &keys {
        for right in &keys {
            let left_bytes = encode_scalar_key(left).unwrap();
            let right_bytes = encode_scalar_key(right).unwrap();
            assert_eq!(
                compare_scalar_keys(&left_bytes, &right_bytes).unwrap(),
                left.cmp(right),
                "{left:?} vs {right:?}"
            );
        }
    }
}

#[test]
fn numeric_equal_across_representations_compares_equal() {
    let integer = encode_scalar_key(&ScalarKey::Number(Number::Integer(1))).unwrap();
    let float = encode_scalar_key(&ScalarKey::Number(Number::Float(1.0))).unwrap();
    let decimal = encode_scalar_key(&ScalarKey::Number(Number::ExactDecimal(
        ExactDecimal::parse("1").unwrap(),
    )))
    .unwrap();
    assert_eq!(
        compare_scalar_keys(&integer, &float).unwrap(),
        Ordering::Equal
    );
    assert_eq!(
        compare_scalar_keys(&integer, &decimal).unwrap(),
        Ordering::Equal
    );
    assert_eq!(
        compare_scalar_keys(&float, &decimal).unwrap(),
        Ordering::Equal
    );
}

#[test]
fn large_string_keys_round_trip() {
    let long = "x".repeat(200_000);
    let key = ScalarKey::String(long.clone());
    let encoded = encode_scalar_key(&key).unwrap();
    let decoded = decode_scalar_key(&encoded).unwrap();
    assert_eq!(decoded, key);
    assert!(encoded.len() > 200_000);
}

#[test]
fn malformed_keys_are_rejected() {
    assert!(decode_scalar_key(&[]).is_err());
    assert!(decode_scalar_key(&[99]).is_err());
    assert!(decode_scalar_key(&[3, 2]).is_err());
    assert!(decode_scalar_key(&[4, 0, 0, 0]).is_err());
    let mut invalid_utf8 = vec![0, 1, 0, 0, 0];
    invalid_utf8.push(0xff);
    assert!(decode_scalar_key(&invalid_utf8).is_err());
    assert!(compare_scalar_keys(&[3], &[3, 1]).is_err());
}

#[test]
fn unsigned_order_is_numeric() {
    let low = super::scalar::encode_unsigned(2);
    let high = super::scalar::encode_unsigned(10);
    assert_eq!(
        super::scalar::KeyOrder::Unsigned
            .compare(&low, &high)
            .unwrap(),
        Ordering::Less
    );
}
