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
