//! Scalar text from one validated JSON document, retaining lexical number tokens.

use std::collections::BTreeMap;

use crate::model::{ArrayValue, Number, Value};
use crate::source::TextSegment;

/// JSON permits exactly these four bytes between tokens.
fn json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}


/// Walks validated JSON bytes and records scalar occurrences in source order.
struct ScalarWalker<'a> {
    bytes: &'a [u8],
    pos: usize,
    path: Vec<String>,
    segments: Vec<Option<TextSegment>>,
    resolved_only: bool,
    paths: bool,
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
        self.pos += 1;
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            match byte {
                b'"' => return Ok(start),
                b'\\' => {
                    // The full parser has already validated escape syntax.
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
        let start = self.quote_end()?;
        serde_json::from_slice(&self.bytes[start..self.pos]).map_err(|error| error.to_string())
    }

    fn push(&mut self, text: String) -> Vec<usize> {
        let index = self.segments.len();
        self.segments.push(Some(TextSegment {
            path: if self.paths {
                self.path.clone()
            } else {
                Vec::new()
            },
            text,
        }));
        if self.resolved_only {
            vec![index]
        } else {
            Vec::new()
        }
    }

    /// Build the logical value and its source-order scalar corpus in one
    /// structural traversal. This path is used only for fresh construction;
    /// query projections still use the field-aware walker below.
    fn build_value(&mut self, depth: usize) -> Result<Value, String> {
        // serde_json rejects the 128th nested container. The root is depth 0.
        if depth >= 128 {
            return Err("JSON nesting resource limit exceeded".into());
        }
        self.whitespace();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                let text = self.quoted()?;
                self.push(text.clone());
                Ok(Value::String(text))
            }
            Some(b'{') => {
                self.pos += 1;
                self.whitespace();
                let mut fields = BTreeMap::new();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Value::Object(fields));
                }
                loop {
                    let name = self.quoted()?;
                    self.whitespace();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected JSON member separator".into());
                    }
                    self.pos += 1;
                    fields.insert(name, self.build_value(depth + 1)?);
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
                Ok(Value::Object(fields))
            }
            Some(b'[') => {
                self.pos += 1;
                self.whitespace();
                let mut values = Vec::new();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(Value::Array(ArrayValue::from_unrestricted(values)));
                }
                loop {
                    values.push(self.build_value(depth + 1)?);
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
                Ok(Value::Array(ArrayValue::from_unrestricted(values)))
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
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    _ => {
                        let number = token
                            .parse::<serde_json::Number>()
                            .map_err(|error| error.to_string())?;
                        Value::Number(Number::from_json(&number)?)
                    }
                };
                self.push(token.to_owned());
                Ok(value)
            }
            _ => Err("expected JSON value".into()),
        }
    }

    /// Return indices of occurrences visible through last-wins object access.
    fn value(&mut self) -> Result<Vec<usize>, String> {
        self.whitespace();
        match self.bytes.get(self.pos).copied() {
            Some(b'"') => {
                let text = self.quoted()?;
                Ok(self.push(text))
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
                Ok(self.push(token.to_owned()))
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
    };
    let value = walker.build_value(0)?;
    walker.whitespace();
    if walker.pos != bytes.len() {
        return Err("unexpected JSON text after root".into());
    }
    Ok((value, walker.segments.into_iter().flatten().collect()))
}

/// Extract decoded scalar values from a previously validated document.
/// Unscoped search sees all duplicate-name occurrences; field search sees
/// only the last occurrence of each object member.
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
