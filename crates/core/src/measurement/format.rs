//! Human-facing diagnostic formatting shared by core callers.

/// Format a nonnegative duration as milliseconds rounded to three decimals.
/// Internal measurements retain their nanosecond precision.
pub fn milliseconds_3(nanoseconds: u128) -> String {
    let microseconds = nanoseconds.saturating_add(500) / 1_000;
    format!("{}.{:03}", microseconds / 1_000, microseconds % 1_000)
}

/// Format a signed Unix-epoch timestamp in milliseconds to three decimals.
pub fn signed_milliseconds_3(nanoseconds: i128) -> String {
    if nanoseconds < 0 {
        format!("-{}", milliseconds_3(nanoseconds.unsigned_abs()))
    } else {
        milliseconds_3(nanoseconds as u128)
    }
}

#[cfg(test)]
mod tests {
    super::super::tests::format_tests!();
}
