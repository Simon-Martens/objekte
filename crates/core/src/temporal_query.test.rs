use super::*;

#[test]
fn calendar_and_offset_periods_use_written_precision() {
    let month = PeriodLiteral::parse("2024-02").unwrap();
    let days = month.for_type(&LogicalType::Date).unwrap();
    assert!(days.matches(&Value::Date(date("2024-02-29").unwrap()), Operator::Eq));
    assert!(!days.matches(&Value::Date(date("2024-03-01").unwrap()), Operator::Eq));
    assert!(days.matches(&Value::Date(date("2024-03-01").unwrap()), Operator::Gt));
    assert!(days.matches(&Value::Date(date("2024-02-01").unwrap()), Operator::Le));

    let minute = PeriodLiteral::parse("2026-11-06T14:30+01:00").unwrap();
    let utc = minute.for_type(&LogicalType::Datetime).unwrap();
    let during = Value::parse_typed_string("2026-11-06T13:30:59Z", &LogicalType::Datetime).unwrap();
    let after = Value::parse_typed_string("2026-11-06T13:31:00Z", &LogicalType::Datetime).unwrap();
    assert!(utc.matches(&during, Operator::Eq));
    assert!(utc.matches(&after, Operator::Gt));
    assert!(!utc.matches(&after, Operator::Eq));
    assert!(minute.for_type(&LogicalType::Date).is_err());

    let instant = PeriodLiteral::parse("2026-11-06T14:30:15.250+01:00").unwrap();
    assert_eq!(instant.end_ns - instant.start_ns, 1_000_000);
    assert!(PeriodLiteral::parse("2026-11-06T14:30:15.250Z").is_ok());
}

#[test]
fn unix_seconds_round_up_fractional_and_negative_bounds() {
    let second = PeriodLiteral::parse("1969-12-31T23:59:59Z").unwrap();
    let bounds = second.for_type(&LogicalType::Unixtime).unwrap();
    assert!(bounds.matches(&Value::Unixtime(-1), Operator::Eq));
    assert!(!bounds.matches(&Value::Unixtime(0), Operator::Eq));
    let subsecond = PeriodLiteral::parse("1969-12-31T23:59:59.5Z").unwrap();
    let bounds = subsecond.for_type(&LogicalType::Unixtime).unwrap();
    assert_eq!(bounds.start, Value::Unixtime(0));
    assert_eq!(bounds.end, Value::Unixtime(0));
    assert!(!bounds.matches(&Value::Unixtime(-1), Operator::Eq));
    assert!(!bounds.matches(&Value::Unixtime(0), Operator::Eq));
    assert!(bounds.matches(&Value::Unixtime(0), Operator::Ne));
}

#[test]
fn invalid_calendar_and_timestamp_forms_fail() {
    for text in [
        "2025-02-29",
        "2026-13",
        "0000-01",
        "2026-11-06T14:30",
        "2026-11-06T25:00Z",
        "2026-11-06T14:30:60Z",
        "2026-11-06T14:30:15.1234567890Z",
        "2026-11-06T14:30+24:00",
    ] {
        assert!(PeriodLiteral::parse(text).is_err(), "{text}");
    }
}
