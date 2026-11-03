use super::*;

#[test]
fn periods_cover_leap_days_months_offsets_and_precisions() {
    let month = parse("2024-02").unwrap();
    assert_eq!(month.end - month.start, 29 * 86_400_000_000_000);
    assert_eq!(parse("2024-02-29").unwrap().end, month.end);
    assert!(parse("2023-02-29").is_err());
    assert_eq!(parse("1970-01-01").unwrap().start, 0);
    assert_eq!(parse("1969-12-31").unwrap().end, 0);
    assert_eq!(
        parse("2026-12-04T14:30Z").unwrap().end - parse("2026-12-04T14:30Z").unwrap().start,
        60_000_000_000
    );
    assert_eq!(
        parse("2026-12-04T15:30+01:00").unwrap().start,
        parse("2026-12-04T14:30Z").unwrap().start
    );
    assert_eq!(
        parse("2026-12-04T14:30:01.25Z").unwrap().end
            - parse("2026-12-04T14:30:01.25Z").unwrap().start,
        10_000_000
    );
    assert!(parse("2026-12-04T14:30").is_err());
}

#[test]
fn comparisons_use_the_whole_period() {
    let may = parse("2026-05").unwrap();
    assert!(compare(may.start, super::super::Op::Eq, may));
    assert!(compare(may.end - 1, super::super::Op::Le, may));
    assert!(compare(may.end, super::super::Op::Gt, may));
    assert!(!compare(may.end, super::super::Op::Eq, may));
    assert!(compare(may.start - 1, super::super::Op::Lt, may));
}

#[test]
fn civil_dates_round_trip_across_eras() {
    for days in (-800_000..800_000).step_by(37) {
        let (year, month, day) = civil_from_days(days);
        assert_eq!(days_from_civil(year, month, day), days, "{days}");
        assert!((1..=12).contains(&month) && (1..=month_days(year, month)).contains(&day));
    }
}

fn macro_value(name: &str, now: i128) -> i128 {
    match macro_operand(name, now).unwrap_or_else(|| panic!("{name}")) {
        super::super::Operand::Temporal(period) => {
            assert_eq!(period.end - period.start, 1, "{name}");
            period.start
        }
        super::super::Operand::Literal(value) => i128::from(value.as_i64().unwrap()),
        _ => unreachable!(),
    }
}

#[test]
fn now_macros_resolve_in_utc() {
    // A leap day just before midnight, which was a Thursday.
    let now = parse("2024-02-29T23:59:59.5Z").unwrap().start;
    let day = parse("2024-02-29").unwrap();
    let month = parse("2024-02").unwrap();
    for (name, expected) in [
        ("now", now),
        ("second", 59),
        ("minute", 59),
        ("hour", 23),
        ("weekday", 4),
        ("day", 29),
        ("month", 2),
        ("year", 2024),
        ("yesterday", now - DAY),
        ("tomorrow", parse("2024-03-01T23:59:59.5Z").unwrap().start),
        ("todayStart", day.start),
        ("todayEnd", day.end - 1),
        ("monthStart", month.start),
        ("monthEnd", month.end - 1),
        ("yearStart", parse("2024-01").unwrap().start),
        ("yearEnd", parse("2024-12").unwrap().end - 1),
    ] {
        assert_eq!(macro_value(name, now), expected, "{name}");
    }
    // December rolls the month end into the next year.
    let december = parse("2026-12-31T08:00Z").unwrap().start;
    assert_eq!(
        macro_value("monthEnd", december),
        parse("2027-01-01").unwrap().start - 1
    );
    assert_eq!(macro_value("weekday", december), 4);
    // Before the epoch: 1969-12-31 was a Wednesday.
    let before = parse("1969-12-31T12:00Z").unwrap().start;
    assert_eq!(macro_value("weekday", before), 3);
    assert_eq!(macro_value("day", before), 31);
    assert_eq!(macro_value("hour", before), 12);
    assert!(macro_operand("today", now).is_none());
}
