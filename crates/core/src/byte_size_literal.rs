//! Exact decimal byte-unit operands for numeric FILTER comparisons.

/// Parse a size token into an integer byte count. Ordinary numeric and field
/// operands return `None`; a numeric token ending in letters must use a known
/// case-sensitive unit.
pub(crate) fn parse_size_literal(input: &str) -> Result<Option<i128>, String> {
    let unsigned = input.trim_start_matches(['+', '-']);
    if !unsigned.starts_with(|ch: char| ch.is_ascii_digit())
        || !input.ends_with(|ch: char| ch.is_ascii_alphabetic())
    {
        return Ok(None);
    }
    let unit_start = input
        .find(|ch: char| ch.is_ascii_alphabetic())
        .ok_or("missing byte-size unit")?;
    let (coefficient, unit) = input.split_at(unit_start);
    let factor: u128 = match unit {
        "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1_024,
        "MiB" => 1_048_576,
        "GiB" => 1_073_741_824,
        "TiB" => 1_099_511_627_776,
        _ => return Err(format!("unknown byte-size unit {unit:?}")),
    };
    if coefficient.starts_with('-') {
        return Err("byte-size literal must be nonnegative".into());
    }
    let coefficient = coefficient.strip_prefix('+').unwrap_or(coefficient);
    let (whole, fraction) = coefficient.split_once('.').unwrap_or((coefficient, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (coefficient.contains('.') && fraction.is_empty())
    {
        return Err("invalid decimal byte-size coefficient".into());
    }

    // Multiply the coefficient's decimal digits by the unit factor, starting
    // at its least significant digit. The fractional positions must all be
    // zero in the product. This avoids floating-point rounding and handles
    // arbitrarily many fractional zeros without an overflowing denominator.
    let mut carry = 0_u128;
    let mut position = 0_usize;
    let mut quotient_reversed = Vec::with_capacity(input.len());
    let mut output_digit = |digit: u8| -> Result<(), String> {
        if position < fraction.len() {
            if digit != 0 {
                return Err("byte-size literal is not a whole number of bytes".into());
            }
        } else {
            quotient_reversed.push(digit);
        }
        position += 1;
        Ok(())
    };
    for digit in fraction.bytes().rev().chain(whole.bytes().rev()) {
        let product = u128::from(digit - b'0') * factor + carry;
        output_digit((product % 10) as u8)?;
        carry = product / 10;
    }
    while carry != 0 {
        output_digit((carry % 10) as u8)?;
        carry /= 10;
    }
    let bytes = quotient_reversed
        .iter()
        .rev()
        .try_fold(0_i128, |value, digit| {
            value
                .checked_mul(10)
                .and_then(|value| value.checked_add(i128::from(*digit)))
                .ok_or("byte-size literal exceeds i128 range".to_owned())
        })?;
    Ok(Some(bytes))
}

#[cfg(test)]
#[path = "byte_size_literal.test.rs"]
mod tests;
