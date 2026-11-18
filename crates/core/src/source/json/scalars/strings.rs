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

/// Decode one quoted token into reusable storage, preserving complete scalars.
pub(super) fn with_quoted_text<R>(
    bytes: &[u8],
    token: Range<usize>,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<R, String> {
    let (end, result) = with_next_quoted_text(bytes, token.start, decoded, inspect)?;
    (end == token.end)
        .then_some(result)
        .ok_or_else(|| "invalid JSON string range".into())
}

/// Locate and decode one source string in one pass, without moving scratch.
pub(super) fn with_next_quoted_text<R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    with_next_quoted_text_strategy::<2, _>(bytes, start, decoded, inspect)
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
pub(super) fn with_next_quoted_text_strategy<const STRATEGY: u8, R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    if bytes.get(start) != Some(&b'"') {
        return Err("expected JSON string".into());
    }
    let mut cursor = start + 1;
    let first = next_quoted_delimiter::<0>(bytes, cursor)?;
    if bytes[first] == b'"' {
        let text = std::str::from_utf8(&bytes[cursor..first]).map_err(|error| error.to_string())?;
        return inspect(text).map(|result| (first + 1, result));
    }
    decoded.clear();
    let mut literal_start = cursor;
    let mut delimiter = first;
    loop {
        decoded.push_str(
            std::str::from_utf8(&bytes[literal_start..delimiter])
                .map_err(|error| error.to_string())?,
        );
        if bytes[delimiter] == b'"' {
            return inspect(decoded).map(|result| (delimiter + 1, result));
        }
        cursor = delimiter + 1;
        decoded.push(decode_quoted_escape(bytes, &mut cursor)?);
        cursor += 1;
        literal_start = cursor;
        delimiter = next_quoted_delimiter::<STRATEGY>(bytes, cursor)?;
    }
}
