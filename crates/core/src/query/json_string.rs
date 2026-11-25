//! JSON string tokens for quoted query arguments and field paths.

/// Write a complete JSON string literal into an existing buffer.
pub fn write_json_string_literal(output: &mut String, value: &str) {
    const HEX: &[u8] = b"0123456789abcdef";
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0}'..='\u{1f}' => {
                let byte = ch as usize;
                output.push_str("\\u00");
                output.push(HEX[byte >> 4] as char);
                output.push(HEX[byte & 0xf] as char);
            }
            _ => output.push(ch),
        }
    }
    output.push('"');
}

/// Decode exactly one double-quoted JSON string literal.
pub fn unescape_json_string(token: &str) -> Result<String, String> {
    let content = token
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or("expected a double-quoted string")?;
    let mut chars = content.chars();
    let mut output = String::with_capacity(content.len());
    while let Some(ch) = chars.next() {
        let decoded = match ch {
            '"' | '\u{0}'..='\u{1f}' => return Err("unescaped character in string".into()),
            '\\' => match chars.next().ok_or("trailing string escape")? {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let high = read_hex_quad(&mut chars)?;
                    let scalar = if let 0xd800..=0xdbff = high {
                        if chars.next() != Some('\\') || chars.next() != Some('u') {
                            return Err("expected a low surrogate escape".into());
                        }
                        let low = read_hex_quad(&mut chars)?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            return Err("invalid low surrogate escape".into());
                        }
                        0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                    } else {
                        high
                    };
                    char::from_u32(scalar).ok_or("invalid Unicode scalar escape")?
                }
                _ => return Err("unknown string escape".into()),
            },
            _ => ch,
        };
        output.push(decoded);
    }
    Ok(output)
}

fn read_hex_quad(chars: &mut std::str::Chars<'_>) -> Result<u32, String> {
    let mut value = 0;
    for _ in 0..4 {
        let digit = chars
            .next()
            .and_then(|ch| ch.to_digit(16))
            .ok_or("invalid Unicode escape")?;
        value = (value << 4) | digit;
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
