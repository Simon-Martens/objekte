use super::*;

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
