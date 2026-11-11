//! Scalar text and projected logical values from JSON, retaining lexical number tokens.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use smallvec::SmallVec;

use crate::model::{ArrayValue, Number, Value};
use crate::query::Quantifier;
use crate::reference::{
    BoundFts, BoundTerm, match_borrowed_source_filter, match_borrowed_source_segment,
};
use crate::source::{ScanProjection, TextSegment};

#[cfg(test)]
thread_local! {
    static OWNED_VALUE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OWNED_SEGMENT_PUSHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OWNED_SEGMENT_TEXT_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn owned_query_construction_counts() -> (usize, usize) {
    (
        OWNED_VALUE_VISITS.with(std::cell::Cell::get),
        OWNED_SEGMENT_PUSHES.with(std::cell::Cell::get),
    )
}

#[cfg(test)]
pub(crate) fn owned_segment_text_bytes() -> usize {
    OWNED_SEGMENT_TEXT_BYTES.with(std::cell::Cell::get)
}

/// JSON permits exactly these four bytes between tokens.
fn json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}


/// Walks JSON bytes and records scalar occurrences in source order.
struct ScalarWalker<'a> {
    bytes: &'a [u8],
    pos: usize,
    path: Vec<String>,
    segments: Vec<Option<TextSegment>>,
    resolved_only: bool,
    paths: bool,
    collect_text: bool,
    validate: bool,
}


impl ScalarWalker<'_> {
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .copied()
            .is_some_and(json_whitespace)
        {
            self.pos += 1;
        }
    }

    fn quote_end(&mut self) -> Result<usize, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("expected JSON string".into());
        }
        let search = self.pos + 1;
        let offset = memchr::memchr2(b'"', b'\\', &self.bytes[search..])
            .ok_or("unterminated JSON string")?;
        let delimiter = search + offset;
        if self.bytes[delimiter] == b'"' {
            // Token decoding validates escapes when the string is used.
            // A trusted skipped value intentionally avoids that work.
            self.pos = delimiter + 1;
            return Ok(start);
        }
        // Escape-heavy strings make repeated delimiter searches expensive.
        // Resume the byte loop after the first escape.
        self.pos = delimiter
            .checked_add(2)
            .ok_or("JSON string offset overflow")?;
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            match byte {
                b'"' => return Ok(start),
                b'\\' => {
                    self.pos = self
                        .pos
                        .checked_add(1)
                        .ok_or("JSON string offset overflow")?;
                }
                _ => {}
            }
        }
        Err("unterminated JSON string".into())
    }

    fn quoted(&mut self) -> Result<String, String> {
        if !self.validate {
            // Decode only this required string token. The stream offset avoids
            // rescanning escapes and never validates the rest of the record.
            let mut values =
                serde_json::Deserializer::from_slice(&self.bytes[self.pos..]).into_iter::<String>();
            let value = values
                .next()
                .ok_or("expected JSON string")?
                .map_err(|error| error.to_string())?;
            self.pos += values.byte_offset();
            return Ok(value);
        }
        let start = self.quote_end()?;
        serde_json::from_slice(&self.bytes[start..self.pos]).map_err(|error| error.to_string())
    }

    fn push(&mut self, text: String) -> Option<usize> {
        if !self.collect_text {
            return None;
        }

        #[cfg(test)]
        OWNED_SEGMENT_PUSHES.with(|count| count.set(count.get() + 1));
        #[cfg(test)]
        OWNED_SEGMENT_TEXT_BYTES.with(|count| count.set(count.get() + text.len()));
        let index = self.segments.len();
        self.segments.push(Some(TextSegment {
            path: if self.paths {
                self.path.clone()
            } else {
                Vec::new()
            },
            text,
        }));
        self.resolved_only.then_some(index)
    }

    /// Skip a value whose contents are not required from a record already
    /// validated by an explicit index operation. Delimiters and strings still
    /// have to be located so the enclosing object can find later members.
    fn skip_trusted_value(&mut self, depth: usize) -> Result<(), String> {
        self.whitespace();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                self.quote_end()?;
                Ok(())
            }
            Some(open @ (b'{' | b'[')) => {
                let mut closing = vec![if open == b'{' { b'}' } else { b']' }];
                self.pos += 1;
                while let Some(byte) = self.bytes.get(self.pos).copied() {
                    match byte {
                        b'"' => {
                            self.quote_end()?;
                        }
                        b'{' | b'[' => {
                            if depth.saturating_add(closing.len()) >= 128 {
                                return Err("JSON recursion limit exceeded (resource limit)".into());
                            }
                            closing.push(if byte == b'{' { b'}' } else { b']' });
                            self.pos += 1;
                        }
                        b'}' | b']' => {
                            if closing.pop() != Some(byte) {
                                return Err("mismatched JSON container terminator".into());
                            }
                            self.pos += 1;
                            if closing.is_empty() {
                                return Ok(());
                            }
                        }
                        _ => self.pos += 1,
                    }
                }
                Err("unterminated JSON container".into())
            }
            Some(_) => {
                let start = self.pos;
                while self.bytes.get(self.pos).is_some_and(|byte| {
                    !json_whitespace(*byte) && !matches!(byte, b',' | b']' | b'}')
                }) {
                    self.pos += 1;
                }
                (self.pos > start)
                    .then_some(())
                    .ok_or_else(|| "expected JSON value".into())
            }
            None => Err("expected JSON value".into()),
        }
    }

    /// Construct only requested logical values while collecting scalar text
    /// in the same structural traversal. Build and query use this parser.
    fn build_value(
        &mut self,
        depth: usize,
        retain: bool,
        selected_top: Option<&HashSet<String>>,
    ) -> Result<Option<Value>, String> {
        #[cfg(test)]
        OWNED_VALUE_VISITS.with(|count| count.set(count.get() + 1));
        // serde_json rejects the 128th nested container. The root is depth 0.
        if depth >= 128 {
            return Err("JSON recursion limit exceeded (resource limit)".into());
        }
        self.whitespace();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                let text = self.quoted()?;
                if self.collect_text {
                    let value = retain.then(|| Value::String(text.clone()));
                    self.push(text);
                    Ok(value)
                } else {
                    Ok(retain.then_some(Value::String(text)))
                }
            }
            Some(b'{') => {
                self.pos += 1;
                self.whitespace();
                let mut fields = retain.then(BTreeMap::new);
                let mut last_ranges = (self.collect_text && self.resolved_only)
                    .then(BTreeMap::<String, Range<usize>>::new);
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(fields.map(Value::Object));
                }
                loop {
                    let name = self.quoted()?;
                    self.whitespace();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected JSON member separator".into());
                    }
                    self.pos += 1;
                    let keep_child =
                        retain && selected_top.is_none_or(|selected| selected.contains(&name));
                    if self.paths && self.collect_text {
                        self.path.push(name.clone());
                    }
                    let start = self.segments.len();
                    let child = if !keep_child && !self.collect_text && !self.validate {
                        self.skip_trusted_value(depth + 1)?;
                        None
                    } else {
                        self.build_value(depth + 1, keep_child, None)?
                    };
                    if self.paths && self.collect_text {
                        self.path.pop();
                    }
                    if let Some(last_ranges) = &mut last_ranges
                        && let Some(previous) =
                            last_ranges.insert(name.clone(), start..self.segments.len())
                    {
                        for segment in &mut self.segments[previous] {
                            *segment = None;
                        }
                    }
                    if let (Some(fields), Some(child)) = (&mut fields, child) {
                        fields.insert(name, child);
                    }
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON object terminator".into()),
                    }
                    self.whitespace();
                }
                Ok(fields.map(Value::Object))
            }
            Some(b'[') => {
                self.pos += 1;
                self.whitespace();
                let mut values = retain.then(Vec::new);
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(
                        values.map(|values| Value::Array(ArrayValue::from_unrestricted(values)))
                    );
                }
                loop {
                    let child = self.build_value(depth + 1, retain, None)?;
                    if let (Some(values), Some(child)) = (&mut values, child) {
                        values.push(child);
                    }
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON array terminator".into()),
                    }
                    self.whitespace();
                }
                Ok(values.map(|values| Value::Array(ArrayValue::from_unrestricted(values))))
            }
            Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => {
                let start = self.pos;
                while self.bytes.get(self.pos).is_some_and(|byte| {
                    !json_whitespace(*byte) && !matches!(byte, b',' | b']' | b'}')
                }) {
                    self.pos += 1;
                }
                let token = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|error| error.to_string())?;
                let value = match token {
                    "true" => retain.then_some(Value::Bool(true)),
                    "false" => retain.then_some(Value::Bool(false)),
                    "null" => retain.then_some(Value::Null),
                    _ => {
                        if retain || self.validate {
                            let number = token
                                .parse::<serde_json::Number>()
                                .map_err(|error| error.to_string())?;
                            if retain {
                                Some(Value::Number(Number::from_json(&number)?))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                };
                if self.collect_text {
                    self.push(token.to_owned());
                }
                Ok(value)
            }
            _ => Err("expected JSON value".into()),
        }
    }

    /// Return indices of occurrences visible through last-wins object access.
    #[cfg(test)]
    fn value(&mut self) -> Result<Vec<usize>, String> {
        self.whitespace();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                let text = self.quoted()?;
                Ok(self.push(text).into_iter().collect())
            }
            Some(b'{') => {
                self.pos += 1;
                self.whitespace();
                let mut last_members = self.resolved_only.then(BTreeMap::<String, Vec<usize>>::new);
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Vec::new());
                }
                loop {
                    let name = if self.paths || self.resolved_only {
                        self.quoted()?
                    } else {
                        self.quote_end()?;
                        String::new()
                    };
                    self.whitespace();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected JSON member separator".into());
                    }
                    self.pos += 1;
                    if self.paths {
                        self.path.push(name.clone());
                    }
                    let selected = self.value()?;
                    if self.paths {
                        self.path.pop();
                    }
                    if let Some(last_members) = &mut last_members {
                        last_members.insert(name, selected);
                    }
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON object terminator".into()),
                    }
                    self.whitespace();
                }
                Ok(last_members.map_or_else(Vec::new, |members| {
                    members.into_values().flatten().collect()
                }))
            }
            Some(b'[') => {
                self.pos += 1;
                self.whitespace();
                let mut visible = Vec::new();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(visible);
                }
                loop {
                    if self.resolved_only {
                        visible.extend(self.value()?);
                    } else {
                        self.value()?;
                    }
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON array terminator".into()),
                    }
                }
                Ok(visible)
            }
            Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => {
                let start = self.pos;
                while self.bytes.get(self.pos).is_some_and(|byte| {
                    !json_whitespace(*byte) && !matches!(byte, b',' | b']' | b'}')
                }) {
                    self.pos += 1;
                }
                let token = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|error| error.to_string())?;
                Ok(self.push(token.to_owned()).into_iter().collect())
            }
            _ => Err("expected JSON value".into()),
        }
    }
}


const MAX_SCAN_SCRATCH_CAPACITY: usize = 256 * 1024;


/// Reusable decoded text and provisional decisions owned by one scan worker.
#[derive(Default)]
pub(crate) struct BorrowedScanScratch {
    decoded_text: String,
    events: Vec<BorrowedEvent>,
}


impl BorrowedScanScratch {
    fn trim(&mut self) {
        self.decoded_text.clear();
        self.events.clear();
        if self.decoded_text.capacity() > MAX_SCAN_SCRATCH_CAPACITY {
            self.decoded_text = String::new();
        }
        if self
            .events
            .capacity()
            .saturating_mul(std::mem::size_of::<BorrowedEvent>())
            > MAX_SCAN_SCRATCH_CAPACITY
        {
            self.events = Vec::new();
        }
    }
}


/// One scalar's provisional match state before object duplicates are resolved.
#[derive(Default)]
struct BorrowedEvent {
    visible: bool,
    fts_hit: bool,
    filter_selected: bool,
    filter_hit: bool,
    evidence: Vec<String>,
}


/// Result of matching borrowed source segments in one trusted record.
pub(crate) struct BorrowedScanResult {
    pub(crate) fts_match: bool,
    pub(crate) filter_match: bool,
    pub(crate) evidence: HashSet<String>,
}


/// Query bindings supported by the direct source consumer.
pub(crate) struct BorrowedScanQuery<'a> {
    pub(crate) fts: Option<&'a BoundFts>,
    pub(crate) filter: Option<&'a BoundTerm>,
    pub(crate) decoded_scalars: bool,
    pub(crate) retain_evidence: bool,
}


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
fn with_quoted_text<R>(
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
fn with_next_quoted_text<R>(
    bytes: &[u8],
    start: usize,
    decoded: &mut String,
    inspect: impl FnOnce(&str) -> Result<R, String>,
) -> Result<(usize, R), String> {
    if bytes.get(start) != Some(&b'"') {
        return Err("expected JSON string".into());
    }
    let mut cursor = start + 1;
    let first = memchr::memchr2(b'"', b'\\', &bytes[cursor..])
        .map(|offset| cursor + offset)
        .ok_or("unterminated JSON string")?;
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
        let escape = *bytes.get(cursor).ok_or("short JSON escape")?;
        match escape {
            b'"' => decoded.push('"'),
            b'\\' => decoded.push('\\'),
            b'/' => decoded.push('/'),
            b'b' => decoded.push('\u{0008}'),
            b'f' => decoded.push('\u{000c}'),
            b'n' => decoded.push('\n'),
            b'r' => decoded.push('\r'),
            b't' => decoded.push('\t'),
            b'u' => {
                let first = decode_hex_quad(bytes, cursor + 1)?;
                cursor += 4;
                let scalar = if (0xD800..=0xDBFF).contains(&first) {
                    if bytes.get(cursor + 1..cursor + 3) != Some(&b"\\u"[..]) {
                        return Err("missing JSON low surrogate".into());
                    }
                    let second = decode_hex_quad(bytes, cursor + 3)?;
                    if !(0xDC00..=0xDFFF).contains(&second) {
                        return Err("invalid JSON low surrogate".into());
                    }
                    cursor += 6;
                    0x10000 + ((first - 0xD800) << 10) + second - 0xDC00
                } else {
                    first
                };
                decoded.push(char::from_u32(scalar).ok_or("invalid JSON Unicode scalar")?);
            }
            _ => return Err("invalid JSON escape".into()),
        }
        cursor += 1;
        literal_start = cursor;
        delimiter = memchr::memchr2(b'"', b'\\', &bytes[cursor..])
            .map(|offset| cursor + offset)
            .ok_or("unterminated JSON string")?;
    }
}

fn path_is(path: &[Cow<'_, str>], expected: &[String]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_ref() == expected)
}

fn path_prefix(path: &[Cow<'_, str>], expected: &[String]) -> bool {
    path.len() < expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_ref() == expected)
}


impl<'a> ScalarWalker<'a> {
    #[allow(clippy::too_many_arguments)]
    fn scan_borrowed_value(
        &mut self,
        depth: usize,
        path: &mut SmallVec<[Cow<'a, str>; 8]>,
        decoded: &mut String,
        events: &mut Vec<BorrowedEvent>,
        query: &BorrowedScanQuery<'_>,
        suppress_filter: bool,
        filter_element: bool,
    ) -> Result<(), String> {
        if depth >= 128 {
            return Err("JSON recursion limit exceeded (resource limit)".into());
        }
        self.whitespace();
        let selected_filter = filter_element
            || (!suppress_filter
                && query
                    .filter
                    .is_some_and(|term| path_is(path, &term.path.segments)));
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                let fts_here = query.fts.filter(|fts| {
                    fts.fields.is_empty()
                        || fts
                            .fields
                            .iter()
                            .any(|field| path_is(path, &field.path.segments))
                });
                if !selected_filter && fts_here.is_none() {
                    self.quote_end()?;
                    return Ok(());
                }
                let mut event = BorrowedEvent {
                    visible: true,
                    filter_selected: selected_filter,
                    ..Default::default()
                };
                let (end, ()) = with_next_quoted_text(self.bytes, self.pos, decoded, |text| {
                    if let Some(fts) = fts_here {
                        event.fts_hit = match_borrowed_source_segment(
                            fts,
                            path,
                            text,
                            &mut event.evidence,
                            query.retain_evidence,
                        )?;
                    }
                    if selected_filter {
                        event.filter_hit = match_borrowed_source_filter(
                            query.filter.expect("selected filter"),
                            text,
                        );
                    }
                    Ok(())
                })?;
                self.pos = end;
                events.push(event);
            }
            Some(b'{') => {
                if selected_filter {
                    events.push(BorrowedEvent {
                        visible: true,
                        filter_selected: true,
                        ..Default::default()
                    });
                }
                self.pos += 1;
                self.whitespace();
                let unscoped_value_fts =
                    !query.decoded_scalars && query.fts.is_some_and(|fts| fts.fields.is_empty());
                let resolve_here = unscoped_value_fts
                    || (!suppress_filter
                        && query
                            .filter
                            .is_some_and(|term| path_prefix(path, &term.path.segments)))
                    || query.fts.is_some_and(|fts| {
                        fts.fields
                            .iter()
                            .any(|field| path_prefix(path, &field.path.segments))
                    });
                let mut previous = resolve_here.then(BTreeMap::<String, Range<usize>>::new);
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    let key_start = self.quote_end()?;
                    let key_end = self.pos;
                    let key = if previous.is_some()
                        || query.retain_evidence
                        || query.filter.is_some()
                        || query.fts.is_some_and(|fts| !fts.fields.is_empty())
                    {
                        if self.bytes[key_start + 1..key_end - 1].contains(&b'\\') {
                            with_quoted_text(self.bytes, key_start..key_end, decoded, |name| {
                                Ok(Cow::Owned(name.to_owned()))
                            })?
                        } else {
                            Cow::Borrowed(
                                std::str::from_utf8(&self.bytes[key_start + 1..key_end - 1])
                                    .map_err(|error| error.to_string())?,
                            )
                        }
                    } else {
                        Cow::Borrowed("")
                    };
                    self.whitespace();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected JSON member separator".into());
                    }
                    self.pos += 1;
                    let start = events.len();
                    path.push(key);
                    self.scan_borrowed_value(
                        depth + 1,
                        path,
                        decoded,
                        events,
                        query,
                        suppress_filter || selected_filter,
                        false,
                    )?;
                    let key = path.pop().expect("object key");
                    let track_key = unscoped_value_fts
                        || (!suppress_filter
                            && query.filter.is_some_and(|term| {
                                path_prefix(path, &term.path.segments)
                                    && term.path.segments[path.len()] == key.as_ref()
                            }))
                        || query.fts.is_some_and(|fts| {
                            fts.fields.iter().any(|field| {
                                path_prefix(path, &field.path.segments)
                                    && field.path.segments[path.len()] == key.as_ref()
                            })
                        });
                    if let Some(previous) = &mut previous
                        && track_key
                        && let Some(range) = previous.insert(key.into_owned(), start..events.len())
                    {
                        for event in &mut events[range] {
                            event.visible = false;
                        }
                    }
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON object terminator".into()),
                    }
                    self.whitespace();
                }
            }
            Some(b'[') => {
                let selected_multi_array = selected_filter
                    && query.filter.is_some_and(|term| term.multi)
                    && !filter_element;
                if selected_filter && !selected_multi_array {
                    events.push(BorrowedEvent {
                        visible: true,
                        filter_selected: true,
                        ..Default::default()
                    });
                }
                self.pos += 1;
                self.whitespace();
                if self.bytes.get(self.pos) == Some(&b']') {
                    if selected_multi_array
                        || (!suppress_filter
                            && query.filter.is_some_and(|term| {
                                term.multi && path_prefix(path, &term.path.segments)
                            }))
                    {
                        events.push(BorrowedEvent {
                            visible: true,
                            filter_selected: true,
                            filter_hit: query
                                .filter
                                .is_some_and(|term| term.quantifier != Quantifier::Any),
                            ..Default::default()
                        });
                    }
                    self.pos += 1;
                    return Ok(());
                }
                loop {
                    self.scan_borrowed_value(
                        depth + 1,
                        path,
                        decoded,
                        events,
                        query,
                        suppress_filter || selected_filter,
                        selected_multi_array,
                    )?;
                    self.whitespace();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err("expected JSON array terminator".into()),
                    }
                    self.whitespace();
                }
            }
            Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => {
                let start = self.pos;
                while self.bytes.get(self.pos).is_some_and(|byte| {
                    !json_whitespace(*byte) && !matches!(byte, b',' | b']' | b'}')
                }) {
                    self.pos += 1;
                }
                let token = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|error| error.to_string())?;
                if selected_filter || (query.decoded_scalars && query.fts.is_some()) {
                    let mut event = BorrowedEvent {
                        visible: true,
                        filter_selected: selected_filter,
                        ..Default::default()
                    };
                    if query.decoded_scalars
                        && let Some(fts) = query.fts
                    {
                        event.fts_hit = match_borrowed_source_segment(
                            fts,
                            path,
                            token,
                            &mut event.evidence,
                            query.retain_evidence,
                        )?;
                    }
                    events.push(event);
                }
            }
            _ => return Err("expected JSON value".into()),
        }
        Ok(())
    }
}


/// Scan source-owned scalar slices, decoding only escaped strings into worker scratch.
pub(crate) fn scan_borrowed_segments(
    bytes: &[u8],
    query: &BorrowedScanQuery<'_>,
    scratch: &mut BorrowedScanScratch,
) -> Result<BorrowedScanResult, String> {
    scratch.decoded_text.clear();
    scratch.events.clear();
    let mut walker = ScalarWalker {
        bytes,
        pos: 0,
        path: Vec::new(),
        segments: Vec::new(),
        resolved_only: false,
        paths: false,
        collect_text: false,
        validate: false,
    };
    let mut path = SmallVec::new();
    walker.scan_borrowed_value(
        0,
        &mut path,
        &mut scratch.decoded_text,
        &mut scratch.events,
        query,
        false,
        false,
    )?;
    let unscoped_decoded =
        query.decoded_scalars && query.fts.is_some_and(|fts| fts.fields.is_empty());
    let mut fts_match = query.fts.is_none();
    let mut evidence = HashSet::new();
    for event in &scratch.events {
        if event.fts_hit && (event.visible || unscoped_decoded) {
            fts_match = true;
            evidence.extend(event.evidence.iter().cloned());
        }
    }
    let filter_match = if let Some(term) = query.filter {
        let mut selected = scratch
            .events
            .iter()
            .filter(|event| event.visible && event.filter_selected);
        if term.multi {
            let first = selected.next();
            if term.quantifier == Quantifier::Any {
                first.is_some_and(|event| event.filter_hit)
                    || selected.any(|event| event.filter_hit)
            } else {
                first.is_some_and(|event| event.filter_hit)
                    && selected.all(|event| event.filter_hit)
            }
        } else {
            selected.next().is_some_and(|event| event.filter_hit)
        }
    } else {
        true
    };
    scratch.events.clear();
    Ok(BorrowedScanResult {
        fts_match,
        filter_match,
        evidence,
    })
}

thread_local! {
    static BORROWED_SCAN_SCRATCH: RefCell<BorrowedScanScratch> =
        RefCell::new(BorrowedScanScratch::default());
}

#[cfg(test)]
static BORROWED_SCAN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn borrowed_scan_calls() -> usize {
    BORROWED_SCAN_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Preserve decoder and provisional-match capacities across a worker's files.
pub(crate) fn scan_borrowed_segments_worker(
    bytes: &[u8],
    query: &BorrowedScanQuery<'_>,
) -> Result<BorrowedScanResult, String> {
    #[cfg(test)]
    BORROWED_SCAN_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    BORROWED_SCAN_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let result = scan_borrowed_segments(bytes, query, &mut scratch);
        scratch.trim();
        result
    })
}

/// Parse one complete document once while retaining all scalar occurrences.
/// Object fields still resolve to their last occurrence in the logical value.
pub(crate) fn build_record(bytes: &[u8]) -> Result<(Value, Vec<TextSegment>), String> {
    let mut walker = ScalarWalker {
        bytes,
        pos: 0,
        path: Vec::new(),
        segments: Vec::new(),
        resolved_only: false,
        paths: false,
        collect_text: true,
        validate: true,
    };
    let value = walker
        .build_value(0, true, None)?
        .ok_or("root JSON value missing")?;
    walker.whitespace();
    if walker.pos != bytes.len() {
        return Err("unexpected JSON text after root".into());
    }
    Ok((value, walker.segments.into_iter().flatten().collect()))
}

/// Extract query values and decoded scalar text in one structural traversal.
/// Trusted records were validated by an explicit index operation, so skipped
/// fields and trailing bytes are not parsed again merely to check freshness.
pub(crate) fn extract_query_record(
    bytes: &[u8],
    projection: &ScanProjection,
    trusted: bool,
) -> Result<(Value, Vec<TextSegment>), String> {
    extract_query_requirements(
        bytes,
        projection.top_level.as_ref(),
        projection.collect_text,
        projection.resolved_text_only,
        trusted,
    )
}

/// Parse only the logical projection for readers whose text comes from the
/// resolved value rather than from every decoded source scalar.
pub(crate) fn extract_query_value(
    bytes: &[u8],
    projection: &ScanProjection,
    trusted: bool,
) -> Result<Value, String> {
    extract_query_requirements(bytes, projection.top_level.as_ref(), false, false, trusted)
        .map(|(value, _)| value)
}

fn extract_query_requirements(
    bytes: &[u8],
    selected_top: Option<&HashSet<String>>,
    collect_text: bool,
    resolved_text_only: bool,
    trusted: bool,
) -> Result<(Value, Vec<TextSegment>), String> {
    let mut walker = ScalarWalker {
        bytes,
        pos: 0,
        path: Vec::new(),
        segments: Vec::new(),
        resolved_only: resolved_text_only,
        paths: collect_text,
        collect_text,
        validate: !trusted,
    };
    let value = walker
        .build_value(0, true, selected_top)?
        .ok_or("root JSON value missing")?;
    if !trusted {
        walker.whitespace();
        if walker.pos != bytes.len() {
            return Err("unexpected JSON text after root".into());
        }
    }
    Ok((value, walker.segments.into_iter().flatten().collect()))
}

/// Extract decoded scalar values from a previously validated document.
/// Unscoped search sees all duplicate-name occurrences; field search sees
/// only the last occurrence of each object member.
#[cfg(test)]
pub(crate) fn scalar_segments(
    bytes: &[u8],
    resolved_only: bool,
    paths: bool,
) -> Result<Vec<TextSegment>, String> {
    let mut walker = ScalarWalker {
        bytes,
        pos: 0,
        path: Vec::new(),
        segments: Vec::new(),
        resolved_only,
        paths,
        collect_text: true,
        validate: false,
    };
    let visible = walker.value()?;
    walker.whitespace();
    if walker.pos != bytes.len() {
        return Err("unexpected JSON text after root".into());
    }
    if !resolved_only {
        return Ok(walker.segments.into_iter().flatten().collect());
    }
    let mut keep = vec![false; walker.segments.len()];
    for index in visible {
        keep[index] = true;
    }
    Ok(walker
        .segments
        .into_iter()
        .enumerate()
        .filter_map(|(index, segment)| keep[index].then_some(segment).flatten())
        .collect())
}

#[cfg(test)]
#[path = "json_scalar_text.test.rs"]
mod tests;
