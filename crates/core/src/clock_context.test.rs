use super::*;
use time::format_description::well_known::Rfc3339;

fn frozen() -> ClockContext {
    ClockContext::new(OffsetDateTime::parse("2026-11-08T23:59:59.5+02:00", &Rfc3339).unwrap())
}

fn period(operand: Operand) -> PeriodLiteral {
    let Operand::Period(period) = operand else {
        panic!("expected a temporal period")
    };
    period
}

fn integer(operand: Operand) -> i128 {
    let Operand::Literal(Literal::Number(Number::Integer(number))) = operand else {
        panic!("expected a numeric component")
    };
    number
}

#[test]
fn frozen_clock_resolves_utc_components_and_periods() {
    let clock = frozen();
    // The explicit +02:00 input is Sunday locally but Sunday 21:59 UTC too.
    assert_eq!(integer(clock.resolve("second").unwrap()), 59);
    assert_eq!(integer(clock.resolve("minute").unwrap()), 59);
    assert_eq!(integer(clock.resolve("hour").unwrap()), 21);
    assert_eq!(integer(clock.resolve("weekday").unwrap()), 0);
    assert_eq!(integer(clock.resolve("day").unwrap()), 8);
    assert_eq!(integer(clock.resolve("month").unwrap()), 11);
    assert_eq!(integer(clock.resolve("year").unwrap()), 2026);

    let now = period(clock.resolve("now").unwrap());
    assert_eq!(now.end_ns - now.start_ns, 1);
    assert_eq!(
        period(clock.resolve("yesterday").unwrap()).start_ns,
        now.start_ns - DAY_NS
    );
    assert_eq!(
        period(clock.resolve("tomorrow").unwrap()).start_ns,
        now.start_ns + DAY_NS
    );
    let today = period(clock.resolve("today").unwrap());
    assert_eq!(today.end_ns - today.start_ns, DAY_NS);
    assert_eq!(
        period(clock.resolve("todayStart").unwrap()).start_ns,
        today.start_ns
    );
    assert_eq!(
        period(clock.resolve("todayEnd").unwrap()).start_ns,
        today.end_ns - 1
    );
    let month = period(clock.resolve("thisMonth").unwrap());
    assert_eq!(
        period(clock.resolve("monthStart").unwrap()).start_ns,
        month.start_ns
    );
    assert_eq!(
        period(clock.resolve("monthEnd").unwrap()).start_ns,
        month.end_ns - 1
    );
    let year = period(clock.resolve("thisYear").unwrap());
    assert_eq!(
        period(clock.resolve("yearStart").unwrap()).start_ns,
        year.start_ns
    );
    assert_eq!(
        period(clock.resolve("yearEnd").unwrap()).start_ns,
        year.end_ns - 1
    );
    assert!(
        clock
            .resolve("unknown")
            .unwrap_err()
            .contains("unknown time macro")
    );
}
