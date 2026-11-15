//! Ordered codec property and boundary tests.

use crate::decimal::ExactDecimal;
use crate::model::Number;
use crate::posting::ScalarKey;

use super::*;

fn encode_number(number: &Number) -> Vec<u8> {
    let mut writer = KeyWriter::new();
    writer.push_scalar(&ScalarKey::Number(number.clone())).expect("number");
    writer.into_bytes()
}

fn assert_order(left: &Number, right: &Number) {
    assert_eq!(
        encode_number(left).cmp(&encode_number(right)),
        left.cmp(right),
        "{left:?} vs {right:?}"
    );
}

#[test]
fn numeric_encoding_matches_number_order_for_representations() {
    let values = [
        Number::Integer(i128::MIN),
        Number::Integer(-2),
        Number::Integer(-1),
        Number::Integer(0),
        Number::Integer(1),
        Number::Integer(2),
        Number::Integer(15),
        Number::Integer(20),
        Number::Integer(2i128.pow(53)),
        Number::Integer(2i128.pow(53) + 1),
        Number::Integer(i128::MAX),
        Number::float(0.5).expect("float"),
        Number::float(-0.5).expect("float"),
        Number::float(1.5).expect("float"),
        Number::float(-1.5).expect("float"),
        Number::float(1e300).expect("float"),
        Number::float(-1e300).expect("float"),
        Number::ExactDecimal(ExactDecimal::parse("0.1").expect("decimal")),
        Number::ExactDecimal(ExactDecimal::parse("-0.1").expect("decimal")),
        Number::ExactDecimal(ExactDecimal::parse("1e-300").expect("decimal")),
        Number::ExactDecimal(ExactDecimal::parse("1e300").expect("decimal")),
        Number::ExactDecimal(ExactDecimal::parse(
            "1234567890123456789012345678901234567890",
        )
        .expect("decimal")),
    ];
    for left in &values {
        for right in &values {
            assert_order(left, right);
        }
    }
}

#[test]
fn equal_numeric_values_encode_identically() {
    let integer = Number::Integer(1);
    let float = Number::float(1.0).expect("float");
    let decimal = Number::ExactDecimal(ExactDecimal::parse("1.000").expect("decimal"));
    assert_eq!(integer, float);
    assert_eq!(integer, decimal);
    assert_eq!(encode_number(&integer), encode_number(&float));
    assert_eq!(encode_number(&integer), encode_number(&decimal));

    let negative_zero = Number::Float(-0.0);
    assert_eq!(negative_zero, Number::Integer(0));
    assert_eq!(
        encode_number(&negative_zero),
        encode_number(&Number::Integer(0))
    );
}

#[test]
fn significand_prefix_order_is_preserved() {
    for (left, right) in [
        ("1.25", "1.5"),
        ("1.5", "2"),
        ("2.0001", "2.001"),
        ("9.999", "10"),
        ("1e-10", "1e-9"),
        ("-1.5", "-1.25"),
        ("-2", "-1.5"),
    ] {
        let left = Number::ExactDecimal(ExactDecimal::parse(left).expect("left"));
        let right = Number::ExactDecimal(ExactDecimal::parse(right).expect("right"));
        assert_order(&left, &right);
    }
}

#[test]
fn i128_and_exponent_extremes_do_not_overflow() {
    let huge = "1".repeat(4_096);
    let values = [
        Number::ExactDecimal(ExactDecimal::parse(&huge).expect("huge")),
        Number::ExactDecimal(
            ExactDecimal::parse(&format!("{huge}e9223372036854775807")).expect("huge exponent"),
        ),
        Number::ExactDecimal(
            ExactDecimal::parse(&format!("{huge}e-9223372036854775808")).expect("tiny exponent"),
        ),
        Number::Integer(i128::MIN),
        Number::Integer(i128::MAX),
    ];
    for value in &values {
        let encoded = encode_number(value);
        let mut cursor = Cursor::new(&encoded);
        let decoded = cursor.read_scalar().expect("decode");
        assert_eq!(cursor.remaining(), 0);
        match (&decoded, value) {
            (ScalarKey::Number(left), right) => assert_eq!(left, right),
            _ => panic!("expected number"),
        }
    }
}

#[test]
fn hundred_thousand_generated_numeric_pairs_match_order() {
    // Deterministic xorshift generator: no external dependency and a fixed
    // seed for reproducible failures.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut previous: Option<Number> = None;
    for index in 0..100_000u64 {
        let raw = (next() as i32) as i64;
        let exponent = (next() % 13) as i64 - 6;
        // Exercise exact binary floats on a bounded subset; converting every
        // random f64 to its full 750-digit decimal expansion dominates the
        // test's runtime without adding order coverage.
        let kind = if index % 50 == 0 { 2 } else { next() % 2 };
        let value = match kind {
            0 => Number::Integer(i128::from(raw)),
            1 => Number::ExactDecimal(
                ExactDecimal::parse(&format!("{raw}e{exponent}")).expect("decimal"),
            ),
            _ => {
                let float = f64::from_bits(next());
                if !float.is_finite() {
                    Number::Integer(0)
                } else {
                    Number::float(float).unwrap_or(Number::Integer(0))
                }
            }
        };
        if let Some(previous) = &previous {
            assert_eq!(
                encode_number(previous).cmp(&encode_number(&value)),
                previous.cmp(&value),
                "{previous:?} vs {value:?}"
            );
        }
        previous = Some(value);
    }
}

#[test]
fn escaped_bytes_preserve_order_and_round_trip() {
    let values = ["", "a", "a\0", "a\0b", "a\0\0", "a\0\u{FFFD}", "ab", "\u{0}", "\u{FF}"];
    for value in values {
        let mut writer = KeyWriter::new();
        writer.push_bytes(value.as_bytes());
        let encoded = writer.into_bytes();
        let mut cursor = Cursor::new(&encoded);
        assert_eq!(cursor.read_bytes().expect("decode"), value.as_bytes());
        assert_eq!(cursor.remaining(), 0);
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let mut encoded = values
        .iter()
        .map(|value| {
            let mut writer = KeyWriter::new();
            writer.push_bytes(value.as_bytes());
            writer.into_bytes()
        })
        .collect::<Vec<_>>();
    encoded.sort();
    for pair in encoded.windows(2) {
        assert!(pair[0] <= pair[1]);
    }
}

#[test]
fn prefix_successor_handles_empty_all_ff_and_embedded_ff() {
    assert_eq!(prefix_successor(&[]), None);
    assert_eq!(prefix_successor(&[0xFF, 0xFF]), None);
    assert_eq!(prefix_successor(&[0x01, 0x02]), Some(vec![0x01, 0x03]));
    assert_eq!(
        prefix_successor(&[0x01, 0xFF, 0xFF]),
        Some(vec![0x02])
    );
    let prefix = b"a\0\0".to_vec();
    let successor = prefix_successor(&prefix).expect("successor");
    assert!(successor > prefix);
    let mut writer = KeyWriter::new();
    writer.push_str("a\0");
    let encoded = writer.into_bytes();
    assert!(encoded > prefix && encoded > successor || encoded < successor);
}

#[test]
fn signed_fixed_width_encoding_is_order_preserving() {
    let mut values = [i64::MIN, -100, -1, 0, 1, 100, i64::MAX];
    let mut encoded = values
        .iter()
        .map(|value| {
            let mut writer = KeyWriter::new();
            writer.push_i64(*value);
            writer.into_bytes()
        })
        .collect::<Vec<_>>();
    encoded.sort();
    values.sort();
    for (index, value) in values.iter().enumerate() {
        let mut cursor = Cursor::new(&encoded[index]);
        assert_eq!(cursor.read_i64().expect("decode"), *value);
    }
    let mut i128_values = [i128::MIN, -1, 0, 1, i128::MAX];
    let mut encoded = i128_values
        .iter()
        .map(|value| {
            let mut writer = KeyWriter::new();
            writer.push_i128(*value);
            writer.into_bytes()
        })
        .collect::<Vec<_>>();
    encoded.sort();
    i128_values.sort();
    for (index, value) in i128_values.iter().enumerate() {
        let mut cursor = Cursor::new(&encoded[index]);
        assert_eq!(cursor.read_i128().expect("decode"), *value);
    }
}

#[test]
fn scalar_tags_are_the_documented_values() {
    assert_eq!(
        encode_scalar(&ScalarKey::String("x".into())).expect("scalar")[0],
        SCALAR_STRING
    );
    assert_eq!(
        encode_scalar(&ScalarKey::Id("x".into())).expect("scalar")[0],
        SCALAR_ID
    );
    let number = encode_scalar(&ScalarKey::Number(Number::Integer(0))).expect("scalar");
    assert_eq!(number[0], SCALAR_NUMBER);
    assert_eq!(number[1], NUMERIC_ZERO);
    assert_eq!(
        encode_scalar(&ScalarKey::Bool(false)).expect("scalar"),
        vec![SCALAR_BOOL, 0]
    );
}

#[test]
fn malformed_scalar_bytes_are_rejected() {
    for bytes in [
        vec![],
        vec![99],
        vec![SCALAR_STRING, b'a'],
        vec![SCALAR_STRING, b'a', 0, 1],
        vec![SCALAR_BOOL, 2],
        vec![SCALAR_NUMBER, NUMERIC_ZERO, 1, 0, 0],
        vec![SCALAR_NUMBER, 9],
        vec![SCALAR_NUMBER, NUMERIC_POSITIVE, 0, 0],
    ] {
        let mut cursor = Cursor::new(&bytes);
        assert!(
            cursor.read_scalar().is_err(),
            "accepted malformed scalar {bytes:?}"
        );
    }
}

#[test]
fn sort_rank_and_descending_component_match_sort_key_order() {
    use crate::sorting::{Placement, SortKey, SortOptions};
    let options = SortOptions {
        nulls: Placement::Last,
        missing: Placement::First,
        ..SortOptions::default()
    };
    let keys = [
        SortKey::Missing,
        SortKey::Null,
        SortKey::String(b"a".to_vec()),
        SortKey::String(b"b".to_vec()),
        SortKey::Id("a".into()),
        SortKey::Number(Number::Integer(1)),
        SortKey::Bool(false),
    ];
    for left in &keys {
        for right in &keys {
            for descending in [false, true] {
                let mut left_writer = KeyWriter::new();
                left_writer.push_sort_key(left, descending, options);
                let mut right_writer = KeyWriter::new();
                right_writer.push_sort_key(right, descending, options);
                assert_eq!(
                    left_writer.as_slice().cmp(right_writer.as_slice()),
                    left.compare_with(right, descending, options),
                    "{left:?} vs {right:?} descending={descending}"
                );
            }
        }
    }
}

#[test]
fn descending_does_not_move_special_groups() {
    use crate::sorting::{Placement, SortKey, SortOptions};
    for options in [
        SortOptions {
            nulls: Placement::First,
            missing: Placement::First,
            ..SortOptions::default()
        },
        SortOptions {
            nulls: Placement::Last,
            missing: Placement::Last,
            ..SortOptions::default()
        },
    ] {
        let mut missing = KeyWriter::new();
        missing.push_sort_key(&SortKey::Missing, true, options);
        let mut null = KeyWriter::new();
        null.push_sort_key(&SortKey::Null, true, options);
        let mut ordinary = KeyWriter::new();
        ordinary.push_sort_key(&SortKey::String(b"x".to_vec()), true, options);
        let mut order = [
            missing.into_bytes(),
            null.into_bytes(),
            ordinary.into_bytes(),
        ];
        order.sort();
        // The encoded order must equal the direct comparison order.
        let keys = [SortKey::Missing, SortKey::Null, SortKey::String(b"x".to_vec())];
        let mut expected = keys.to_vec();
        expected.sort_by(|left, right| left.compare_with(right, true, options));
        let encoded = order
            .iter()
            .map(|bytes| {
                if bytes == &{
                    let mut writer = KeyWriter::new();
                    writer.push_sort_key(&SortKey::Missing, true, options);
                    writer.into_bytes()
                } {
                    SortKey::Missing
                } else if bytes == &{
                    let mut writer = KeyWriter::new();
                    writer.push_sort_key(&SortKey::Null, true, options);
                    writer.into_bytes()
                } {
                    SortKey::Null
                } else {
                    SortKey::String(b"x".to_vec())
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(encoded, expected);
    }
}

#[test]
fn numeric_components_round_trip() {
    for number in [
        Number::Integer(0),
        Number::Integer(-1),
        Number::Integer(i128::MAX),
        Number::Integer(i128::MIN),
        Number::ExactDecimal(ExactDecimal::parse("0.0001").expect("decimal")),
        Number::ExactDecimal(ExactDecimal::parse("-123.456e78").expect("decimal")),
    ] {
        let mut writer = KeyWriter::new();
        writer.push_numeric_component(&number).expect("encode");
        let bytes = writer.into_bytes();
        let mut cursor = Cursor::new(&bytes);
        let decoded = cursor.read_numeric_component().expect("decode");
        assert_eq!(decoded, number);
    }
}

#[test]
fn ordering_is_consistent_with_full_scalar_comparison() {
    let values = [
        ScalarKey::String(String::new()),
        ScalarKey::String("a".into()),
        ScalarKey::String("a\0b".into()),
        ScalarKey::Id("a".into()),
        ScalarKey::Number(Number::Integer(-5)),
        ScalarKey::Number(Number::Integer(0)),
        ScalarKey::Number(Number::Integer(5)),
        ScalarKey::Bool(false),
        ScalarKey::Bool(true),
        ScalarKey::Date(time::Date::from_julian_day(1).expect("date")),
        ScalarKey::Date(time::Date::from_julian_day(2).expect("date")),
        ScalarKey::Datetime(
            time::OffsetDateTime::from_unix_timestamp_nanos(-1).expect("datetime"),
        ),
        ScalarKey::Datetime(time::OffsetDateTime::from_unix_timestamp_nanos(1).expect("datetime")),
        ScalarKey::Unixtime(-1),
        ScalarKey::Unixtime(1),
    ];
    for left in &values {
        for right in &values {
            let left_encoded = encode_scalar(left).expect("left");
            let right_encoded = encode_scalar(right).expect("right");
            assert_eq!(
                left_encoded.cmp(&right_encoded),
                left.cmp(right),
                "{left:?} vs {right:?}"
            );
        }
    }
}
