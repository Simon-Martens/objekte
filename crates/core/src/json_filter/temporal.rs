//! Exact UTC periods for unquoted filter date and time literals.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;


#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(super) struct Period {
    pub start: i128,
    pub end: i128,
}


fn digits(text: &str, width: usize) -> Option<i64> {
    (text.len() == width && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Proleptic Gregorian days relative to the Unix epoch.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * adjusted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn invalid() -> Error {
    Error::Request("invalid UTC date/time literal; use YYYY-MM, YYYY-MM-DD, or an ISO time with Z or an offset".into())
}

pub(super) fn parse(text: &str) -> Result<Period> {
    let year = digits(text.get(..4).ok_or_else(invalid)?, 4).ok_or_else(invalid)?;
    let month = digits(text.get(5..7).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
    if text.as_bytes().get(4) != Some(&b'-')
        || !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
    {
        return Err(invalid());
    }
    let month_start = i128::from(days_from_civil(year, month, 1)) * 86_400_000_000_000;
    if text.len() == 7 {
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        return Ok(Period {
            start: month_start,
            end: i128::from(days_from_civil(next_year, next_month, 1)) * 86_400_000_000_000,
        });
    }
    let day = digits(text.get(8..10).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
    if text.as_bytes().get(7) != Some(&b'-') || day < 1 || day > month_days(year, month) {
        return Err(invalid());
    }
    let day_start = i128::from(days_from_civil(year, month, day)) * 86_400_000_000_000;
    if text.len() == 10 {
        return Ok(Period {
            start: day_start,
            end: day_start + 86_400_000_000_000,
        });
    }
    if text.as_bytes().get(10) != Some(&b'T') {
        return Err(invalid());
    }
    let (clock, offset_seconds) = if let Some(clock) = text.strip_suffix('Z') {
        (clock, 0_i64)
    } else {
        let zone = text[11..]
            .rfind(['+', '-'])
            .map(|index| index + 11)
            .ok_or_else(invalid)?;
        let offset = &text[zone..];
        let hours = digits(offset.get(1..3).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
        let minutes = digits(offset.get(4..6).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
        if offset.len() != 6
            || offset.as_bytes().get(3) != Some(&b':')
            || hours > 23
            || minutes > 59
        {
            return Err(invalid());
        }
        let seconds = (hours * 60 + minutes) * 60;
        (
            &text[..zone],
            if offset.starts_with('-') {
                -seconds
            } else {
                seconds
            },
        )
    };
    let hour = digits(clock.get(11..13).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
    let minute = digits(clock.get(14..16).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
    if clock.as_bytes().get(13) != Some(&b':') || hour > 23 || minute > 59 {
        return Err(invalid());
    }
    let mut second = 0;
    let mut step = 60_000_000_000_i128;
    let mut nanos = 0_i128;
    if clock.len() > 16 {
        if clock.as_bytes().get(16) != Some(&b':') {
            return Err(invalid());
        }
        second = digits(clock.get(17..19).ok_or_else(invalid)?, 2).ok_or_else(invalid)?;
        if second > 59 {
            return Err(invalid());
        }
        step = 1_000_000_000;
        if clock.len() > 19 {
            let fraction = clock.get(20..).ok_or_else(invalid)?;
            if clock.as_bytes().get(19) != Some(&b'.')
                || fraction.is_empty()
                || fraction.len() > 9
                || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(invalid());
            }
            step = 10_i128.pow((9 - fraction.len()) as u32);
            nanos = fraction.parse::<i128>().map_err(|_| invalid())? * step;
        }
    }
    let start = day_start
        + i128::from((hour * 60 + minute) * 60 + second - offset_seconds) * 1_000_000_000
        + nanos;
    Ok(Period {
        start,
        end: start + step,
    })
}

const DAY: i128 = 86_400_000_000_000;


// Proleptic Gregorian civil date of days relative to the Unix epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn day_start(year: i64, month: i64, day: i64) -> i128 {
    i128::from(days_from_civil(year, month, day)) * DAY
}

/// PocketBase-style `@now` macros in UTC at `now` (Unix nanoseconds).
/// Date and time macros are instants; `*End` macros are the last nanosecond
/// of their period, so `<= @todayEnd` covers the whole day. Number macros
/// count from zero for the second, minute, hour, and weekday (Sunday).
pub(super) fn macro_operand(name: &str, now: i128) -> Option<super::Operand> {
    let instant = |at: i128| {
        super::Operand::Temporal(Period {
            start: at,
            end: at + 1,
        })
    };
    let number = |value: i128| super::Operand::Literal(Value::from(value as i64));
    let days = now.div_euclid(DAY);
    let since_midnight = now.rem_euclid(DAY);
    let (year, month, day) = civil_from_days(days as i64);
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Some(match name {
        "now" => instant(now),
        "second" => number(since_midnight / 1_000_000_000 % 60),
        "minute" => number(since_midnight / 60_000_000_000 % 60),
        "hour" => number(since_midnight / 3_600_000_000_000),
        "weekday" => number((days + 4).rem_euclid(7)),
        "day" => number(i128::from(day)),
        "month" => number(i128::from(month)),
        "year" => number(i128::from(year)),
        "yesterday" => instant(now - DAY),
        "tomorrow" => instant(now + DAY),
        "todayStart" => instant(days * DAY),
        "todayEnd" => instant(days * DAY + DAY - 1),
        "monthStart" => instant(day_start(year, month, 1)),
        "monthEnd" => instant(day_start(next_year, next_month, 1) - 1),
        "yearStart" => instant(day_start(year, 1, 1)),
        "yearEnd" => instant(day_start(year + 1, 1, 1) - 1),
        _ => return None,
    })
}

pub(super) fn compare(instant: i128, operator: super::Op, period: Period) -> bool {
    use super::Op;
    match operator {
        Op::Eq => period.start <= instant && instant < period.end,
        Op::Ne => instant < period.start || period.end <= instant,
        Op::Lt => instant < period.start,
        Op::Le => instant < period.end,
        Op::Gt => period.end <= instant,
        Op::Ge => period.start <= instant,
        Op::Contains | Op::NotContains => false,
    }
}

#[cfg(test)]
#[path = "temporal.test.rs"]
mod tests;
