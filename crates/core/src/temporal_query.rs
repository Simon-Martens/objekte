//! Validated, half-open temporal operands for structured FILTER comparisons.

use std::cmp::Ordering;

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::model::{LogicalType, Value};
use crate::query::Operator;

const SECOND_NS: i128 = 1_000_000_000;

/// Recognize the date prefix before the ordinary field-path lexer sees colons.
pub fn looks_like_temporal(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 7
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
}


/// One written calendar or timestamp period, before field-type lowering.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PeriodLiteral {
    pub start_ns: i128,
    pub end_ns: i128,
    pub date_start: Date,
    pub date_end: Option<Date>,
    pub has_time: bool,
}


/// Period endpoints converted to the indexed field's exact logical type.
#[derive(Clone, Debug)]
pub struct TemporalBounds {
    pub start: Value,
    pub end: Value,
}


fn digits(text: &str, start: usize, end: usize) -> Result<u32, String> {
    let value = text.get(start..end).ok_or("invalid temporal literal")?;
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid temporal literal".into());
    }
    value.parse().map_err(|_| "invalid temporal literal".into())
}

fn date(text: &str) -> Result<Date, String> {
    if text.len() != 10
        || text.as_bytes().get(4) != Some(&b'-')
        || text.as_bytes().get(7) != Some(&b'-')
    {
        return Err("invalid temporal date; expected YYYY-MM-DD".into());
    }
    let year = i32::try_from(digits(text, 0, 4)?).map_err(|_| "temporal year out of range")?;
    if year == 0 {
        return Err("temporal year out of range".into());
    }
    let month = u8::try_from(digits(text, 5, 7)?).map_err(|_| "temporal month out of range")?;
    let day = u8::try_from(digits(text, 8, 10)?).map_err(|_| "temporal day out of range")?;
    let month = Month::try_from(month).map_err(|_| "invalid temporal month")?;
    Date::from_calendar_date(year, month, day).map_err(|_| "invalid temporal calendar date".into())
}

fn utc_nanos(date: Date) -> i128 {
    date.midnight().assume_utc().unix_timestamp_nanos()
}


impl PeriodLiteral {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() == 7 {
            if text.as_bytes().get(4) != Some(&b'-') {
                return Err("invalid temporal month; expected YYYY-MM".into());
            }
            let year =
                i32::try_from(digits(text, 0, 4)?).map_err(|_| "temporal year out of range")?;
            if year == 0 {
                return Err("temporal year out of range".into());
            }
            let month =
                u8::try_from(digits(text, 5, 7)?).map_err(|_| "temporal month out of range")?;
            let month = Month::try_from(month).map_err(|_| "invalid temporal month")?;
            let start = Date::from_calendar_date(year, month, 1)
                .map_err(|_| "invalid temporal calendar month")?;
            let (next_year, next_month) = if month == Month::December {
                (
                    year.checked_add(1).ok_or("temporal month end overflow")?,
                    Month::January,
                )
            } else {
                (year, month.next())
            };
            let end = Date::from_calendar_date(next_year, next_month, 1)
                .map_err(|_| "temporal month end out of range")?;
            return Ok(Self {
                start_ns: utc_nanos(start),
                end_ns: utc_nanos(end),
                date_start: start,
                date_end: Some(end),
                has_time: false,
            });
        }
        if text.len() == 10 {
            let start = date(text)?;
            let end = start.next_day().ok_or("temporal day end out of range")?;
            return Ok(Self {
                start_ns: utc_nanos(start),
                end_ns: utc_nanos(end),
                date_start: start,
                date_end: Some(end),
                has_time: false,
            });
        }
        let (clock, offset) = if let Some(clock) = text.strip_suffix('Z') {
            (clock, UtcOffset::UTC)
        } else {
            let zone = text
                .get(10..)
                .and_then(|tail| tail.rfind(['+', '-']).map(|at| at + 10))
                .ok_or("time-bearing temporal literal needs Z or numeric offset")?;
            let suffix = &text[zone..];
            if suffix.len() != 6 || suffix.as_bytes().get(3) != Some(&b':') {
                return Err("invalid temporal offset".into());
            }
            let hours =
                i8::try_from(digits(suffix, 1, 3)?).map_err(|_| "invalid temporal offset")?;
            let minutes =
                i8::try_from(digits(suffix, 4, 6)?).map_err(|_| "invalid temporal offset")?;
            if hours > 23 || minutes > 59 {
                return Err("invalid temporal offset".into());
            }
            let sign = if suffix.starts_with('-') { -1 } else { 1 };
            let offset = UtcOffset::from_hms(sign * hours, sign * minutes, 0)
                .map_err(|_| "invalid temporal offset")?;
            (&text[..zone], offset)
        };
        if clock.len() < 16
            || clock.as_bytes().get(10) != Some(&b'T')
            || clock.as_bytes().get(13) != Some(&b':')
        {
            return Err("invalid temporal timestamp".into());
        }
        let calendar = date(&clock[..10])?;
        let hour = u8::try_from(digits(clock, 11, 13)?).map_err(|_| "invalid hour")?;
        let minute = u8::try_from(digits(clock, 14, 16)?).map_err(|_| "invalid minute")?;
        let mut second = 0;
        let mut nanosecond = 0;
        let mut step_ns = 60 * SECOND_NS;
        if clock.len() > 16 {
            if clock.as_bytes().get(16) != Some(&b':') || clock.len() < 19 {
                return Err("invalid temporal seconds".into());
            }
            second = u8::try_from(digits(clock, 17, 19)?).map_err(|_| "invalid second")?;
            step_ns = SECOND_NS;
            if clock.len() > 19 {
                let fraction = clock.get(20..).ok_or("invalid temporal fraction")?;
                if clock.as_bytes().get(19) != Some(&b'.')
                    || fraction.is_empty()
                    || fraction.len() > 9
                    || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err("invalid temporal fraction".into());
                }
                step_ns = 10_i128.pow((9 - fraction.len()) as u32);
                nanosecond = u32::try_from(
                    fraction
                        .parse::<i128>()
                        .map_err(|_| "invalid temporal fraction")?
                        * step_ns,
                )
                .map_err(|_| "invalid temporal fraction")?;
            }
        }
        let time = Time::from_hms_nano(hour, minute, second, nanosecond)
            .map_err(|_| "invalid temporal clock time")?;
        let start_ns = PrimitiveDateTime::new(calendar, time)
            .assume_offset(offset)
            .unix_timestamp_nanos();
        let end_ns = start_ns
            .checked_add(step_ns)
            .ok_or("temporal period end overflow")?;
        OffsetDateTime::from_unix_timestamp_nanos(end_ns)
            .map_err(|_| "temporal period end out of range")?;
        Ok(Self {
            start_ns,
            end_ns,
            date_start: calendar,
            date_end: None,
            has_time: true,
        })
    }

    pub fn for_type(&self, kind: &LogicalType) -> Result<TemporalBounds, String> {
        let (start, end) = match kind {
            LogicalType::Date if !self.has_time => (
                Value::Date(self.date_start),
                Value::Date(self.date_end.ok_or("missing calendar period end")?),
            ),
            LogicalType::Date => return Err("time-of-day period cannot compare with Date".into()),
            LogicalType::Datetime => (
                Value::Datetime(
                    OffsetDateTime::from_unix_timestamp_nanos(self.start_ns)
                        .map_err(|_| "temporal start out of range")?,
                ),
                Value::Datetime(
                    OffsetDateTime::from_unix_timestamp_nanos(self.end_ns)
                        .map_err(|_| "temporal end out of range")?,
                ),
            ),
            LogicalType::Unixtime => (
                Value::Unixtime(
                    i64::try_from(
                        self.start_ns.div_euclid(SECOND_NS)
                            + i128::from(self.start_ns.rem_euclid(SECOND_NS) != 0),
                    )
                    .map_err(|_| "Unix second start out of range")?,
                ),
                Value::Unixtime(
                    i64::try_from(
                        self.end_ns.div_euclid(SECOND_NS)
                            + i128::from(self.end_ns.rem_euclid(SECOND_NS) != 0),
                    )
                    .map_err(|_| "Unix second end out of range")?,
                ),
            ),
            _ => return Err(format!("temporal period is incompatible with {kind:?}")),
        };
        Ok(TemporalBounds { start, end })
    }
}


impl TemporalBounds {
    pub fn matches(&self, value: &Value, operator: Operator) -> bool {
        let (before_start, before_end) = match (value, &self.start, &self.end) {
            (Value::Date(value), Value::Date(start), Value::Date(end)) => {
                (value.cmp(start), value.cmp(end))
            }
            (Value::Datetime(value), Value::Datetime(start), Value::Datetime(end)) => {
                (value.cmp(start), value.cmp(end))
            }
            (Value::Unixtime(value), Value::Unixtime(start), Value::Unixtime(end)) => {
                (value.cmp(start), value.cmp(end))
            }
            _ => return false,
        };
        matches_ordering(before_start, before_end, operator)
    }
}


/// Apply a comparison to two already typed half-open endpoints.
pub fn matches_ordering(start: Ordering, end: Ordering, operator: Operator) -> bool {
    match operator {
        Operator::Eq => start != Ordering::Less && end == Ordering::Less,
        Operator::Ne => start == Ordering::Less || end != Ordering::Less,
        Operator::Lt => start == Ordering::Less,
        Operator::Le => end == Ordering::Less,
        Operator::Gt => end != Ordering::Less,
        Operator::Ge => start != Ordering::Less,
        Operator::Contains | Operator::NotContains => false,
    }
}

#[cfg(test)]
#[path = "temporal_query.test.rs"]
mod tests;
