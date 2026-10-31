//! Conservative text representation for source files that are not valid JSON.

/// Keep original text and a best-effort JSON-unescaped copy searchable.
/// Unknown escapes remain literal, so malformed syntax never hides raw text.
pub(crate) fn malformed_search_text(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let decoded = decode_escapes(&raw);
    if decoded == raw {
        raw.into_owned()
    } else {
        format!("{raw}\u{1f}{decoded}")
    }
}

fn decode_escapes(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut result = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 >= bytes.len() {
            let ch = source[index..].chars().next().expect("UTF-8 character");
            result.push(ch);
            index += ch.len_utf8();
            continue;
        }
        if !bytes[index + 1].is_ascii() {
            // A backslash before a multibyte character is not a JSON escape.
            // Advance one byte so the next iteration starts at a UTF-8 boundary.
            result.push('\\');
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        match bytes[start + 1] {
            b'"' => result.push('"'),
            b'\\' => result.push('\\'),
            b'/' => result.push('/'),
            b'b' => result.push('\u{8}'),
            b'f' => result.push('\u{c}'),
            b'n' => result.push('\n'),
            b'r' => result.push('\r'),
            b't' => result.push('\t'),
            b'u' => {
                let Some(first) = hex_word(bytes.get(index..index.saturating_add(4))) else {
                    result.push_str(&source[start..index]);
                    continue;
                };
                index += 4;
                let scalar = if (0xd800..=0xdbff).contains(&first)
                    && bytes.get(index..index + 2) == Some(b"\\u")
                {
                    hex_word(bytes.get(index + 2..index + 6)).and_then(|second| {
                        (0xdc00..=0xdfff).contains(&second).then_some(
                            0x10000
                                + (u32::from(first - 0xd800) << 10)
                                + u32::from(second - 0xdc00),
                        )
                    })
                } else {
                    Some(u32::from(first))
                };
                if let Some(ch) = scalar.and_then(char::from_u32) {
                    result.push(ch);
                    if ch.len_utf8() == 4 && (0xd800..=0xdbff).contains(&first) {
                        index += 6;
                    }
                } else {
                    result.push_str(&source[start..index]);
                }
            }
            _ => result.push_str(&source[start..index]),
        }
    }
    result
}

fn hex_word(bytes: Option<&[u8]>) -> Option<u16> {
    let bytes = bytes?;
    if bytes.len() != 4 {
        return None;
    }
    let mut value = 0u16;
    for &byte in bytes {
        value = (value << 4) | u16::from((byte as char).to_digit(16)? as u8);
    }
    Some(value)
}

#[cfg(test)]
#[path = "malformed.test.rs"]
mod tests;
