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
    use super::{milliseconds_3, signed_milliseconds_3};

    #[test]
    fn durations_have_millisecond_units_and_three_decimals() {
        assert_eq!(milliseconds_3(0), "0.000");
        assert_eq!(milliseconds_3(499), "0.000");
        assert_eq!(milliseconds_3(500), "0.001");
        assert_eq!(milliseconds_3(1_234_567), "1.235");
        assert_eq!(milliseconds_3(2_000_000), "2.000");
        assert_eq!(signed_milliseconds_3(-1_234_567), "-1.235");
    }
}
