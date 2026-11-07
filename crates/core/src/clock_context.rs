//! One frozen UTC clock for resolving all macros in a query invocation.

use time::{Date, Month, OffsetDateTime, UtcOffset};

use crate::model::Number;
use crate::query::{Literal, Operand};
use crate::temporal_query::PeriodLiteral;

const DAY_NS: i128 = 86_400_000_000_000;


/// Execution-scoped UTC instant, injectable by callers and tests.
#[derive(Clone, Copy, Debug)]
pub struct ClockContext {
    pub now: OffsetDateTime,
}


fn next_month(date: Date) -> Result<Date, String> {
    let (year, month) = if date.month() == Month::December {
        (
            date.year().checked_add(1).ok_or("month macro overflow")?,
            Month::January,
        )
    } else {
        (date.year(), date.month().next())
    };
    Date::from_calendar_date(year, month, 1).map_err(|_| "month macro out of range".into())
}

fn next_year(date: Date) -> Result<Date, String> {
    let year = date.year().checked_add(1).ok_or("year macro overflow")?;
    Date::from_calendar_date(year, Month::January, 1).map_err(|_| "year macro out of range".into())
}


impl ClockContext {
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            now: now.to_offset(UtcOffset::UTC),
        }
    }

    /// Resolve a zero-argument macro without consulting the clock again.
    pub fn resolve(&self, name: &str) -> Result<Operand, String> {
        let now = self.now;
        let today = now.date();
        let tomorrow = || today.next_day().ok_or("day macro out of range".to_owned());
        let month_start = || {
            Date::from_calendar_date(today.year(), today.month(), 1)
                .map_err(|_| "month macro out of range".to_owned())
        };
        let year_start = || {
            Date::from_calendar_date(today.year(), Month::January, 1)
                .map_err(|_| "year macro out of range".to_owned())
        };
        let at = |nanos: i128| PeriodLiteral::instant(nanos).map(Operand::Period);
        let number = |value: i128| Operand::Literal(Literal::Number(Number::integer(value)));
        match name {
            "now" => at(now.unix_timestamp_nanos()),
            "yesterday" => at(now
                .unix_timestamp_nanos()
                .checked_sub(DAY_NS)
                .ok_or("yesterday macro overflow")?),
            "tomorrow" => at(now
                .unix_timestamp_nanos()
                .checked_add(DAY_NS)
                .ok_or("tomorrow macro overflow")?),
            "todayStart" => at(today.midnight().assume_utc().unix_timestamp_nanos()),
            "todayEnd" => at(tomorrow()?.midnight().assume_utc().unix_timestamp_nanos() - 1),
            "monthStart" => at(month_start()?
                .midnight()
                .assume_utc()
                .unix_timestamp_nanos()),
            "monthEnd" => at(next_month(today)?
                .midnight()
                .assume_utc()
                .unix_timestamp_nanos()
                - 1),
            "yearStart" => at(year_start()?.midnight().assume_utc().unix_timestamp_nanos()),
            "yearEnd" => at(next_year(today)?
                .midnight()
                .assume_utc()
                .unix_timestamp_nanos()
                - 1),
            "today" => Ok(Operand::Period(PeriodLiteral::calendar(today, tomorrow()?))),
            "thisMonth" => Ok(Operand::Period(PeriodLiteral::calendar(
                month_start()?,
                next_month(today)?,
            ))),
            "thisYear" => Ok(Operand::Period(PeriodLiteral::calendar(
                year_start()?,
                next_year(today)?,
            ))),
            "second" => Ok(number(i128::from(now.second()))),
            "minute" => Ok(number(i128::from(now.minute()))),
            "hour" => Ok(number(i128::from(now.hour()))),
            "weekday" => Ok(number(i128::from(now.weekday().number_days_from_sunday()))),
            "day" => Ok(number(i128::from(now.day()))),
            "month" => Ok(number(i128::from(now.month() as u8))),
            "year" => Ok(number(i128::from(now.year()))),
            _ => Err(format!("unknown time macro @{name}")),
        }
    }
}


#[cfg(test)]
#[path = "clock_context.test.rs"]
mod tests;
