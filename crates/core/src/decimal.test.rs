use super::*;

#[test]
fn decimal_public_parse_and_deserialization_require_canonical_values() {
    for invalid in ["", "-", "abc", "01", "1.", ".1", "1e", "1e+", "1a"] {
        assert!(ExactDecimal::parse(invalid).is_err(), "{invalid:?}");
    }

    for invalid in [
        ExactDecimal {
            negative: true,
            digits: "0".into(),
            exponent: 0,
        },
        ExactDecimal {
            negative: false,
            digits: "10".into(),
            exponent: 0,
        },
        ExactDecimal {
            negative: false,
            digits: "abc".into(),
            exponent: 0,
        },
    ] {
        let bytes = bincode::serialize(&invalid).unwrap();
        assert!(bincode::deserialize::<ExactDecimal>(&bytes).is_err());
    }
    let valid = ExactDecimal::parse("1.00e2").unwrap();
    let bytes = bincode::serialize(&valid).unwrap();
    assert_eq!(bincode::deserialize::<ExactDecimal>(&bytes).unwrap(), valid);
}

#[test]
fn decimal_comparison_stays_compact_across_large_exponents() {
    let huge = ExactDecimal::parse("1e1000000").unwrap();
    let larger = ExactDecimal::parse("2e1000000").unwrap();
    let tiny = ExactDecimal::parse("1e-1000000").unwrap();
    assert_eq!(huge.digits, "1");
    assert_eq!(huge.exponent, 1_000_000);
    assert_eq!(huge.to_string(), "1e1000000");
    assert_eq!(huge.cmp_value(&larger), Ordering::Less);
    assert_eq!(
        tiny.cmp_value(&ExactDecimal::from_i128(0)),
        Ordering::Greater
    );
    assert_eq!(
        ExactDecimal::parse("1.00e2").unwrap(),
        ExactDecimal::parse("100").unwrap()
    );
    assert_eq!(
        ExactDecimal::parse("-0e1000000").unwrap(),
        ExactDecimal::parse("0").unwrap()
    );
}

#[test]
fn binary_float_comparison_uses_its_exact_value() {
    let tenth = ExactDecimal::parse("0.1").unwrap();
    let float_tenth = ExactDecimal::from_f64(0.1);
    assert_eq!(tenth.cmp_value(&float_tenth), Ordering::Less);
    assert_eq!(
        ExactDecimal::from_f64(0.5),
        ExactDecimal::parse("0.5").unwrap()
    );
    assert_eq!(
        ExactDecimal::from_f64(f64::from_bits(1)).digits.len() > 700,
        true
    );
}
