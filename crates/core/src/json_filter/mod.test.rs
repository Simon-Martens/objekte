use super::*;

#[test]
fn unmodified_fields_and_literals_borrow_large_values() {
    let document = serde_json::json!({"payload": {"text": "x".repeat(256 * 1024)}});
    let field = FieldOperand {
        path: vec![JsonPathStep::Key("payload".into())],
        modifier: None,
        metadata: None,
    };
    let selected = field.select(&document, None, None);
    assert_eq!(selected.len(), 1);
    assert!(std::ptr::eq(
        selected[0].value().unwrap(),
        &document["payload"]
    ));

    let literal = Operand::Literal(serde_json::json!({"text": "y".repeat(256 * 1024)}));
    let selected = literal.select(&document, None, None);
    let Operand::Literal(value) = &literal else {
        unreachable!()
    };
    assert!(std::ptr::eq(selected[0].value().unwrap(), value));
}

#[test]
fn cache_identity_ignores_boolean_order_and_bound_parameter_spelling() {
    let left = JsonFilter::compile(
        "year = $year && (city = 'Berlin' && name = 'Müller')",
        &[("year".into(), serde_json::json!(1820))].into(),
    )
    .unwrap();
    let right = JsonFilter::compile(
        "name = 'Müller' && city = 'Berlin' && year = 1820",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(left.cache_expression(), right.cache_expression());
    let different = JsonFilter::compile(
        "year = 1830 && city = 'Berlin' && name = 'Müller'",
        &BTreeMap::new(),
    )
    .unwrap();
    assert_ne!(left.cache_expression(), different.cache_expression());
}

#[test]
fn cardinality_counts_only_values_eligible_for_a_modifier() {
    let document = serde_json::json!({"items": [1, "A", false, "B"]});
    let lower = FieldOperand {
        path: vec![JsonPathStep::Key("items".into())],
        modifier: Some(Modifier::Lower),
        metadata: None,
    };
    assert!(lower.more_than_one(&document, None));
    let one = serde_json::json!({"items": [1, "A", false]});
    assert!(!lower.more_than_one(&one, None));

    let length = FieldOperand {
        path: vec![JsonPathStep::Key("items".into())],
        modifier: Some(Modifier::Length),
        metadata: None,
    };
    assert!(!length.more_than_one(&document, None));
    assert_eq!(
        length.select(&document, None, None)[0].value(),
        Some(&Value::from(4))
    );
}

#[test]
fn later_branch_cardinality_is_checked_before_short_circuit() {
    let filter = JsonFilter::compile("name = 'match' || items = 'A'", &BTreeMap::new()).unwrap();
    let document = serde_json::json!({"name": "match", "items": ["A", "B"]});
    assert!(matches!(filter.matches(&document), Err(Error::Request(_))));
}

#[test]
fn unit_literals_compare_exact_byte_counts() {
    let metadata = FileMetadata {
        id: "a",
        path: "data/a.json",
        size: 1_000,
        modified: 0,
        created: None,
        status: RecordStatus::NoSchema,
    };
    for (filter, expected) in [
        ("@size = 1KB", true),
        ("@size = 1KiB", false),
        ("@size < 1KiB", true),
        ("@size > 0.5KB", true),
        ("1KB = @size", true),
    ] {
        let filter = JsonFilter::compile(filter, &BTreeMap::new()).unwrap();
        assert_eq!(
            filter
                .matches_with_metadata(&Value::Null, Some(&metadata))
                .unwrap(),
            expected
        );
    }
}

/// The pre-optimization implementation is intentionally independent of the
/// borrowed-digit comparator above.
fn decimal_cmp_reference(a: &str, b: &str) -> Ordering {
    fn parts(lexeme: &str) -> (bool, String, BigInt) {
        let negative = lexeme.starts_with('-');
        let unsigned = lexeme.strip_prefix('-').unwrap_or(lexeme);
        let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
        let fraction = mantissa.split_once('.').map_or(0, |(_, rest)| rest.len());
        let digits = mantissa.replace('.', "").trim_start_matches('0').to_owned();
        let exponent: BigInt = exponent.parse().unwrap();
        let magnitude = exponent - BigInt::from(fraction) + BigInt::from(digits.len());
        (negative && !digits.is_empty(), digits, magnitude)
    }
    let (an, a, am) = parts(a);
    let (bn, b, bm) = parts(b);
    if an != bn {
        return if an {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let order = if a.is_empty() || b.is_empty() {
        (!a.is_empty()).cmp(&(!b.is_empty()))
    } else {
        am.cmp(&bm).then_with(|| {
            a.bytes()
                .chain(std::iter::repeat(b'0'))
                .zip(b.bytes().chain(std::iter::repeat(b'0')))
                .take(a.len().max(b.len()))
                .find_map(|(a, b)| (a != b).then(|| a.cmp(&b)))
                .unwrap_or(Ordering::Equal)
        })
    };
    if an { order.reverse() } else { order }
}

#[test]
fn borrowed_decimal_comparator_matches_exact_reference() {
    let mut values = vec![
        "0".to_owned(),
        "-0".into(),
        "0.000e999".into(),
        "-0e-999".into(),
        "1".into(),
        "1.0".into(),
        "10e-1".into(),
        "-1".into(),
        "0.000001".into(),
        "1e-6".into(),
        "-0.000001".into(),
        "9007199254740992".into(),
        "9007199254740993".into(),
        "9223372036854775808".into(),
        "-9223372036854775809".into(),
        "1e+00003".into(),
        "1E-00003".into(),
        "1e170141183460469231731687303715884105727".into(),
        "1e170141183460469231731687303715884105728".into(),
        "1e-170141183460469231731687303715884105728".into(),
        "1e-170141183460469231731687303715884105729".into(),
        "1.00000000000000000000000000000000000000000001".into(),
        "1.00000000000000000000000000000000000000000002".into(),
    ];
    for i in 1..=40 {
        values.push(format!(
            "{}{}.{}e{}",
            if i % 3 == 0 { "-" } else { "" },
            i,
            "0".repeat(i),
            i * 2
        ));
    }
    for a in &values {
        for b in &values {
            let actual = decimal_cmp(a, b);
            assert_eq!(actual, decimal_cmp_reference(a, b), "{a} versus {b}");
            assert_eq!(actual, decimal_cmp(b, a).reverse(), "{a} versus {b}");
        }
    }
    for a in &values {
        for b in values.iter().step_by(3) {
            for c in values.iter().step_by(5) {
                if decimal_cmp(a, b).is_le() && decimal_cmp(b, c).is_le() {
                    assert!(decimal_cmp(a, c).is_le(), "{a}, {b}, {c}");
                }
            }
        }
    }
}
