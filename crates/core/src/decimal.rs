//! Compact exact decimal representation for JSON numbers outside native range.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};


/// Signed significant digits times ten to a signed exponent.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct ExactDecimal {
    pub(crate) negative: bool,
    pub(crate) digits: String,
    pub(crate) exponent: i64,
}


/// Wire-compatible fields checked before constructing a public decimal.
#[derive(Deserialize)]
struct DecimalRepresentation {
    negative: bool,
    digits: String,
    exponent: i64,
}


impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let decimal = DecimalRepresentation::deserialize(deserializer)?;
        let digits = decimal.digits.as_bytes();
        let canonical_zero = digits == b"0" && !decimal.negative && decimal.exponent == 0;
        let canonical_nonzero = !digits.is_empty()
            && digits.len() <= 4096
            && digits[0] != b'0'
            && digits[digits.len() - 1] != b'0'
            && digits.iter().all(u8::is_ascii_digit);
        if !canonical_zero && !canonical_nonzero {
            return Err(de::Error::custom("invalid exact decimal representation"));
        }
        Ok(Self {
            negative: decimal.negative,
            digits: decimal.digits,
            exponent: decimal.exponent,
        })
    }
}


impl ExactDecimal {
    pub fn parse(token: &str) -> Result<Self, String> {
        const MAX_SIGNIFICAND_DIGITS: usize = 4096;
        let bytes = token.as_bytes();
        if !valid_json_number_token(bytes) {
            return Err("invalid decimal number".into());
        }
        let mut start = 0;
        let negative = bytes.first() == Some(&b'-');
        if negative {
            start += 1;
        }
        let exponent_at = token[start..].find(['e', 'E']).map(|index| start + index);
        let coefficient_end = exponent_at.unwrap_or(token.len());
        let coefficient = &token[start..coefficient_end];
        if coefficient.len() > MAX_SIGNIFICAND_DIGITS + 1 {
            return Err("JSON number exceeds significant-digit resource limit (4096)".into());
        }
        let dot = coefficient.find('.');
        let fractional = dot.map_or(0, |index| coefficient.len() - index - 1);
        let mut exponent = if let Some(index) = exponent_at {
            token[index + 1..]
                .parse::<i64>()
                .map_err(|_| "JSON exponent exceeds supported resource limit".to_string())?
        } else {
            0
        };
        exponent = exponent
            .checked_sub(fractional as i64)
            .ok_or("JSON exponent exceeds supported resource limit")?;
        let mut digits = coefficient
            .bytes()
            .filter(|byte| *byte != b'.')
            .map(char::from)
            .collect::<String>();
        let first = digits
            .bytes()
            .position(|byte| byte != b'0')
            .unwrap_or(digits.len());
        if first == digits.len() {
            return Ok(Self {
                negative: false,
                digits: "0".into(),
                exponent: 0,
            });
        }
        digits.drain(..first);
        while digits.ends_with('0') {
            digits.pop();
            exponent = exponent
                .checked_add(1)
                .ok_or("JSON exponent exceeds supported resource limit")?;
        }
        Ok(Self {
            negative,
            digits,
            exponent,
        })
    }

    pub(crate) fn from_i128(value: i128) -> Self {
        Self::parse(&value.to_string()).expect("integer is a valid decimal")
    }

    pub(crate) fn from_f64(value: f64) -> Self {
        let bits = value.to_bits();
        let negative = bits >> 63 != 0;
        let exponent_bits = ((bits >> 52) & 0x7ff) as i32;
        let fraction = bits & ((1u64 << 52) - 1);
        let (mantissa, binary_exponent) = if exponent_bits == 0 {
            (fraction, -1074)
        } else {
            (fraction | (1u64 << 52), exponent_bits - 1023 - 52)
        };
        if mantissa == 0 {
            return Self {
                negative: false,
                digits: "0".into(),
                exponent: 0,
            };
        }
        let mut digits = mantissa
            .to_string()
            .bytes()
            .rev()
            .map(|byte| byte - b'0')
            .collect::<Vec<_>>();
        let mut exponent = 0i64;
        if binary_exponent >= 0 {
            for _ in 0..binary_exponent {
                multiply_digits(&mut digits, 2);
            }
        } else {
            for _ in 0..-binary_exponent {
                multiply_digits(&mut digits, 5);
            }
            exponent = binary_exponent as i64;
        }
        let text = digits
            .iter()
            .rev()
            .map(|digit| char::from(b'0' + digit))
            .collect::<String>();
        let mut normalized = Self::parse(&format!("{text}e{exponent}"))
            .expect("finite binary float has bounded decimal exponent");
        normalized.negative = negative;
        normalized
    }

    pub(crate) fn cmp_value(&self, other: &Self) -> Ordering {
        if self.digits == "0" {
            return if other.digits == "0" {
                Ordering::Equal
            } else if other.negative {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        if other.digits == "0" {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if self.negative != other.negative {
            return if self.negative {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let sign = if self.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
        let left_magnitude = self.digits.len() as i128 + self.exponent as i128;
        let right_magnitude = other.digits.len() as i128 + other.exponent as i128;
        let compared = left_magnitude.cmp(&right_magnitude);
        if compared != Ordering::Equal {
            return if sign == Ordering::Greater {
                compared
            } else {
                compared.reverse()
            };
        }
        let left = self.digits.as_bytes();
        let right = other.digits.as_bytes();
        let compared = (0..left.len().max(right.len()))
            .map(|index| {
                left.get(index)
                    .copied()
                    .unwrap_or(b'0')
                    .cmp(&right.get(index).copied().unwrap_or(b'0'))
            })
            .find(|order| *order != Ordering::Equal)
            .unwrap_or(Ordering::Equal);
        if sign == Ordering::Greater {
            compared
        } else {
            compared.reverse()
        }
    }

    pub(crate) fn as_i128(&self) -> Option<i128> {
        if self.exponent < 0 || self.digits.len() as i128 + self.exponent as i128 > 39 {
            return None;
        }
        let mut result = 0i128;
        for byte in self.digits.bytes() {
            result = result.checked_mul(10)?.checked_sub((byte - b'0') as i128)?;
        }
        for _ in 0..self.exponent {
            result = result.checked_mul(10)?;
        }
        if self.negative {
            Some(result)
        } else {
            result.checked_neg()
        }
    }

    pub(crate) fn as_exact_f64(&self) -> Option<f64> {
        let magnitude = self.digits.len() as i128 + self.exponent as i128;
        if !(-324..=309).contains(&magnitude) {
            return None;
        }
        let text = format!(
            "{}{}e{}",
            if self.negative { "-" } else { "" },
            self.digits,
            self.exponent
        );
        let value = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
        (self.cmp_value(&Self::from_f64(value)) == Ordering::Equal).then_some(value)
    }
}


fn valid_json_number_token(bytes: &[u8]) -> bool {
    let mut position = usize::from(bytes.first() == Some(&b'-'));
    match bytes.get(position).copied() {
        Some(b'0') => position += 1,
        Some(b'1'..=b'9') => {
            position += 1;
            while bytes.get(position).is_some_and(u8::is_ascii_digit) {
                position += 1;
            }
        }
        _ => return false,
    }
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let first_fraction = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == first_fraction {
            return false;
        }
    }
    if matches!(bytes.get(position), Some(b'e' | b'E')) {
        position += 1;
        if matches!(bytes.get(position), Some(b'+' | b'-')) {
            position += 1;
        }
        let first_exponent = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == first_exponent {
            return false;
        }
    }
    position == bytes.len()
}


impl fmt::Display for ExactDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            formatter.write_str("-")?;
        }
        write!(formatter, "{}e{}", self.digits, self.exponent)
    }
}


fn multiply_digits(digits: &mut Vec<u8>, multiplier: u8) {
    let mut carry = 0u16;
    for digit in digits.iter_mut() {
        let product = *digit as u16 * multiplier as u16 + carry;
        *digit = (product % 10) as u8;
        carry = product / 10;
    }
    while carry != 0 {
        digits.push((carry % 10) as u8);
        carry /= 10;
    }
}

#[cfg(test)]
#[path = "decimal.test.rs"]
mod tests;
