//! Scalar text and projected logical values from JSON, retaining lexical number tokens.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use crate::model::{ArrayValue, Number, Value};
use crate::query::binding::filter::BoundTerm;
use crate::query::binding::fts::BoundFts;
use crate::reference::fts::match_borrowed_source_segment;
use crate::source::{ScanProjection, TextSegment};

mod scan;
mod strings;

use scan::scan_borrowed_segments;
use strings::with_next_quoted_text;

#[cfg(test)]
thread_local! {
    static OWNED_VALUE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OWNED_SEGMENT_PUSHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OWNED_SEGMENT_TEXT_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static BORROWED_NODE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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

/// Match source scalars while the required logical projection is constructed.
/// The path owns only active member names; scalar text is consumed before the
/// required Value takes ownership or before reusable decoding storage is reset.
struct CombinedFts<'a> {
    fts: &'a BoundFts,
    path: &'a mut Vec<String>,
    decoded: &'a mut String,
    events: &'a mut Vec<BorrowedEvent>,
    retain_evidence: bool,
}

impl CombinedFts<'_> {
    fn push_text(&mut self, text: &str) -> Result<(), String> {
        let mut event = BorrowedEvent {
            visible: true,
            ..Default::default()
        };
        event.fts_hit = match_borrowed_source_segment(
            self.fts,
            self.path.as_slice(),
            text,
            &mut event.evidence,
            self.retain_evidence,
        )?;
        self.events.push(event);
        Ok(())
    }
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
        mut combined: Option<&mut CombinedFts<'_>>,
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
                if !retain && let Some(combined) = combined.as_deref_mut() {
                    let mut event = BorrowedEvent {
                        visible: true,
                        ..Default::default()
                    };
                    let (end, ()) =
                        with_next_quoted_text(self.bytes, self.pos, combined.decoded, |text| {
                            event.fts_hit = match_borrowed_source_segment(
                                combined.fts,
                                combined.path.as_slice(),
                                text,
                                &mut event.evidence,
                                combined.retain_evidence,
                            )?;
                            Ok(())
                        })?;
                    self.pos = end;
                    combined.events.push(event);
                    return Ok(None);
                }
                let text = self.quoted()?;
                if let Some(combined) = combined.as_deref_mut() {
                    combined.push_text(&text)?;
                }
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
                let mut match_last_ranges = combined.as_ref().and_then(|state| {
                    (!state.fts.fields.is_empty()).then(BTreeMap::<String, Range<usize>>::new)
                });
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
                    if let Some(state) = combined.as_deref_mut() {
                        state.path.push(name.clone());
                    }
                    let start = self.segments.len();
                    let match_start = combined.as_ref().map_or(0, |state| state.events.len());
                    let child = if !keep_child && !self.collect_text && !self.validate {
                        if combined.is_some() {
                            self.build_value(depth + 1, false, None, combined.as_deref_mut())?
                        } else {
                            self.skip_trusted_value(depth + 1)?;
                            None
                        }
                    } else {
                        self.build_value(depth + 1, keep_child, None, combined.as_deref_mut())?
                    };
                    if self.paths && self.collect_text {
                        self.path.pop();
                    }
                    if let Some(state) = combined.as_deref_mut() {
                        state.path.pop();
                        if let Some(ranges) = &mut match_last_ranges
                            && let Some(previous) =
                                ranges.insert(name.clone(), match_start..state.events.len())
                        {
                            for event in &mut state.events[previous] {
                                event.visible = false;
                            }
                        }
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
                    let child =
                        self.build_value(depth + 1, retain, None, combined.as_deref_mut())?;
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
                if let Some(combined) = combined.as_deref_mut() {
                    combined.push_text(token)?;
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
    combined_path: Vec<String>,
}

impl BorrowedScanScratch {
    fn trim(&mut self) {
        self.decoded_text.clear();
        self.events.clear();
        self.combined_path.clear();
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

thread_local! {
    static BORROWED_SCAN_SCRATCH: RefCell<BorrowedScanScratch> =
        RefCell::new(BorrowedScanScratch::default());
}

#[cfg(test)]
static BORROWED_SCAN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static COMBINED_SCAN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn borrowed_scan_calls() -> usize {
    BORROWED_SCAN_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn borrowed_node_visits() -> usize {
    BORROWED_NODE_VISITS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn combined_scan_calls() -> usize {
    COMBINED_SCAN_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Preserve decoder and provisional-match capacities across a worker's files.
pub(crate) fn scan_borrowed_segments_worker(
    bytes: &[u8],
    query: &BorrowedScanQuery<'_>,
) -> Result<BorrowedScanResult, String> {
    let _fold_record = crate::query::semantics::folding::FoldRecordGuard::new();
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
        .build_value(0, true, None, None)?
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

/// Build required logical values and match decoded source scalars in one walk.
/// Unscoped decoded FTS retains every occurrence; selected fields retain only
/// events visible after duplicate member replacement.
pub(crate) fn extract_query_record_borrowed_fts_worker(
    bytes: &[u8],
    projection: &ScanProjection,
    fts: &BoundFts,
    retain_evidence: bool,
) -> Result<(Value, BorrowedScanResult), String> {
    let _fold_record = crate::query::semantics::folding::FoldRecordGuard::new();
    #[cfg(test)]
    COMBINED_SCAN_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    BORROWED_SCAN_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        scratch.decoded_text.clear();
        scratch.events.clear();
        scratch.combined_path.clear();
        let BorrowedScanScratch {
            decoded_text,
            events,
            combined_path,
        } = &mut *scratch;
        let mut combined = CombinedFts {
            fts,
            path: combined_path,
            decoded: decoded_text,
            events,
            retain_evidence,
        };
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
        let result = (|| {
            let value = walker
                .build_value(0, true, projection.top_level.as_ref(), Some(&mut combined))?
                .ok_or("root JSON value missing")?;
            let mut fts_match = false;
            let mut evidence = HashSet::new();
            for event in combined.events.iter() {
                if event.fts_hit && (event.visible || fts.fields.is_empty()) {
                    fts_match = true;
                    if retain_evidence {
                        evidence.extend(event.evidence.iter().cloned());
                    }
                }
            }
            Ok((
                value,
                BorrowedScanResult {
                    fts_match,
                    filter_match: true,
                    evidence,
                },
            ))
        })();
        scratch.trim();
        result
    })
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
        .build_value(0, true, selected_top, None)?
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
mod tests;
