use std::ops::Range;

fn decode_hex_digit(byte: u8) -> Result<u32, String> {
    (byte as char)
        .to_digit(16)
        .ok_or_else(|| "invalid JSON Unicode escape".into())
}

fn decode_hex_quad(bytes: &[u8], at: usize) -> Result<u32, String> {
    let token = bytes.get(at..at + 4).ok_or("short JSON Unicode escape")?;
    token.iter().try_fold(0u32, |value, byte| {
        Ok((value << 4) | decode_hex_digit(*byte)?)
    })
}

/// Decode a JSON quoted token at exactly `token`, including both quotes.
///
/// The range must be in bounds and contain one complete string. Invalid UTF-8,
/// raw control characters, invalid escapes and unpaired UTF-16 surrogates are
/// errors. Plain strings borrow `bytes`; escaped strings use reusable `decoded`.
pub fn with_quoted_text<R>(
    bytes: &[u8],
    token: Range<usize>,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<R, String> {
    if token.start >= token.end || token.end > bytes.len() {
        return Err("invalid JSON string range".into());
    }
    with_next_quoted_text_impl::<2, _>(
        &bytes[..token.end],
        token.start,
        Some(token.end),
        true,
        decoded,
        inspect,
    )
    .map(|(_, result)| result)
}

/// Decode the next quoted JSON token beginning at `start`.
///
/// Returns the exclusive byte offset immediately after the closing quote and
/// the callback result. The callback receives borrowed input for unescaped
/// strings and reusable caller scratch for escaped strings. Invalid UTF-8,
/// raw control characters, invalid escapes, unpaired surrogates and truncated
/// strings are errors.
pub fn with_next_quoted_text<R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    with_next_quoted_text_impl::<2, _>(bytes, start, None, true, decoded, inspect)
}

/// Built-in reader entry point for a token whose JSON syntax was validated
/// during framing. It shares decoding without repeating strict syntax checks.
pub(crate) fn with_next_quoted_text_validated<R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    with_next_quoted_text_impl::<2, _>(bytes, start, None, false, decoded, inspect)
}

pub(crate) fn with_quoted_text_validated<R>(
    bytes: &[u8],
    token: Range<usize>,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<R, String> {
    let (end, result) =
        with_next_quoted_text_impl::<2, _>(bytes, token.start, None, false, decoded, inspect)?;
    (end == token.end)
        .then_some(result)
        .ok_or_else(|| "invalid JSON string range".into())
}

fn next_quoted_delimiter<const STRATEGY: u8>(bytes: &[u8], cursor: usize) -> Result<usize, String> {
    if STRATEGY == 1 {
        for (offset, byte) in bytes[cursor..].iter().enumerate() {
            if matches!(byte, b'"' | b'\\') {
                return Ok(cursor + offset);
            }
        }
        Err("unterminated JSON string".into())
    } else {
        let search = if STRATEGY == 2 || STRATEGY == 3 {
            let probe = if STRATEGY == 2 { 8 } else { 4 };
            let probe_end = cursor.saturating_add(probe).min(bytes.len());
            for index in cursor..probe_end {
                if matches!(bytes[index], b'"' | b'\\') {
                    return Ok(index);
                }
            }
            probe_end
        } else {
            cursor
        };
        memchr::memchr2(b'"', b'\\', &bytes[search..])
            .map(|offset| search + offset)
            .ok_or_else(|| "unterminated JSON string".into())
    }
}

#[cfg(test)]
#[path = "json_string_tests.rs"]
mod tests;

fn decode_quoted_escape(bytes: &[u8], cursor: &mut usize) -> Result<char, String> {
    let escape = *bytes.get(*cursor).ok_or("short JSON escape")?;
    match escape {
        b'"' => Ok('"'),
        b'\\' => Ok('\\'),
        b'/' => Ok('/'),
        b'b' => Ok('\u{0008}'),
        b'f' => Ok('\u{000c}'),
        b'n' => Ok('\n'),
        b'r' => Ok('\r'),
        b't' => Ok('\t'),
        b'u' => {
            let first = decode_hex_quad(bytes, *cursor + 1)?;
            *cursor += 4;
            let scalar = if (0xD800..=0xDBFF).contains(&first) {
                if bytes.get(*cursor + 1..*cursor + 3) != Some(&b"\\u"[..]) {
                    return Err("missing JSON low surrogate".into());
                }
                let second = decode_hex_quad(bytes, *cursor + 3)?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err("invalid JSON low surrogate".into());
                }
                *cursor += 6;
                0x10000 + ((first - 0xD800) << 10) + second - 0xDC00
            } else {
                first
            };
            char::from_u32(scalar).ok_or_else(|| "invalid JSON Unicode scalar".into())
        }
        _ => Err("invalid JSON escape".into()),
    }
}

/// Both delimiter strategies share the same decoder. The initial memchr keeps
/// plain tokens borrowed; the adaptive variant checks short literal runs in a
/// byte loop before using memchr for sparse escapes.
#[cfg(test)]
fn with_next_quoted_text_strategy<const STRATEGY: u8, R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    with_next_quoted_text_impl::<STRATEGY, _>(bytes, start, None, false, decoded, inspect)
}

fn with_next_quoted_text_impl<const STRATEGY: u8, R>(
    bytes: &[u8],
    start: usize,
    expected_end: Option<usize>,
    strict: bool,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    if bytes.get(start) != Some(&b'"') {
        return Err("expected JSON string".into());
    }
    let mut cursor = start + 1;
    let first = next_quoted_delimiter::<0>(bytes, cursor)?;
    if bytes[first] == b'"' {
        let literal = &bytes[cursor..first];
        if strict && literal.iter().any(|byte| *byte < 0x20) {
            return Err("unescaped JSON control character".into());
        }
        if expected_end.is_some_and(|end| end != first + 1) {
            return Err("invalid JSON string range".into());
        }
        let text = std::str::from_utf8(literal).map_err(|error| error.to_string())?;
        return inspect(text).map(|result| (first + 1, result));
    }
    decoded.clear();
    let mut literal_start = cursor;
    let mut delimiter = first;
    loop {
        if strict
            && bytes[literal_start..delimiter]
                .iter()
                .any(|byte| *byte < 0x20)
        {
            return Err("unescaped JSON control character".into());
        }
        decoded.push_str(
            std::str::from_utf8(&bytes[literal_start..delimiter])
                .map_err(|error| error.to_string())?,
        );
        if bytes[delimiter] == b'"' {
            if expected_end.is_some_and(|end| end != delimiter + 1) {
                return Err("invalid JSON string range".into());
            }
            return inspect(decoded).map(|result| (delimiter + 1, result));
        }
        cursor = delimiter + 1;
        decoded.push(decode_quoted_escape(bytes, &mut cursor)?);
        cursor += 1;
        literal_start = cursor;
        delimiter = next_quoted_delimiter::<STRATEGY>(bytes, cursor)?;
    }
}
