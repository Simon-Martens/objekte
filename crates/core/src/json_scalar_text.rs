//! Scalar text and projected logical values from JSON, retaining lexical number tokens.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use crate::model::{ArrayValue, Number, Value};
use crate::source::{ScanProjection, TextSegment};

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
