//! Scalar text from one validated JSON document, retaining lexical number tokens.

use std::collections::BTreeMap;

use crate::source::TextSegment;


/// Walks validated JSON bytes and records scalar occurrences in source order.
struct ScalarWalker<'a> {
    bytes: &'a [u8],
    pos: usize,
    path: Vec<String>,
    segments: Vec<Option<TextSegment>>,
}


impl ScalarWalker<'_> {
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err("expected JSON string".into());
        }
        self.pos += 1;
        while let Some(byte) = self.bytes.get(self.pos).copied() {
            self.pos += 1;
            match byte {
                b'"' => {
                    return serde_json::from_slice(&self.bytes[start..self.pos])
                        .map_err(|error| error.to_string());
                }
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

    fn push(&mut self, text: String) -> Vec<usize> {
        let index = self.segments.len();
        self.segments.push(Some(TextSegment {
            path: self.path.clone(),
            text,
        }));
        vec![index]
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
                let mut last_members = BTreeMap::<String, Vec<usize>>::new();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Vec::new());
                }
                loop {
                    let name = self.quoted()?;
                    self.whitespace();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err("expected JSON member separator".into());
                    }
                    self.pos += 1;
                    self.path.push(name.clone());
                    let selected = self.value()?;
                    self.path.pop();
                    last_members.insert(name, selected);
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
                Ok(last_members.into_values().flatten().collect())
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
                    visible.extend(self.value()?);
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
                    !byte.is_ascii_whitespace() && !matches!(byte, b',' | b']' | b'}')
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


/// Extract decoded scalar values from a previously validated document.
/// Unscoped search sees all duplicate-name occurrences; field search sees
/// only the last occurrence of each object member.
pub(crate) fn scalar_segments(
    bytes: &[u8],
    resolved_only: bool,
) -> Result<Vec<TextSegment>, String> {
    let mut walker = ScalarWalker {
        bytes,
        pos: 0,
        path: Vec::new(),
        segments: Vec::new(),
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
