use super::parse_size_literal;

#[test]
fn si_iec_and_fractional_sizes_are_exact() {
    for (input, expected) in [
        ("0B", 0),
        ("1536B", 1536),
        ("2.5KB", 2500),
        ("1.25MB", 1_250_000),
        ("1GB", 1_000_000_000),
        ("1TB", 1_000_000_000_000),
        ("1KiB", 1024),
        ("1.5MiB", 1_572_864),
        ("2GiB", 2_147_483_648),
        ("1TiB", 1_099_511_627_776),
        ("0.0009765625KiB", 1),
        ("0.00000000001818989403545856475830078125TiB", 20),
        ("0.000000000000000000000000000000000000000000000000B", 0),
    ] {
        assert_eq!(
            parse_size_literal(input).unwrap(),
            Some(expected),
            "{input}"
        );
    }
    assert_eq!(
        parse_size_literal("170141183460469231731687303715884105727B").unwrap(),
        Some(i128::MAX)
    );
}

#[test]
fn invalid_sizes_do_not_round_or_wrap() {
    for input in [
        "0.1B",
        "1.2345KB",
        "-1MiB",
        "1mib",
        "1Mb",
        "1PB",
        "1.2.3MiB",
        "1.MiB",
        "170141183460469231731687303715884105728B",
        "1e3B",
    ] {
        assert!(parse_size_literal(input).is_err(), "{input}");
    }
    for input in ["123", "2.5", "1e3", "name", "@file.size"] {
        assert_eq!(parse_size_literal(input).unwrap(), None, "{input}");
    }
}
