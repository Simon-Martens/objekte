use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use smallvec::SmallVec;

use crate::query::Quantifier;
use crate::reference::{
    fts::fts_field_selects_path, match_borrowed_source_filter, match_borrowed_source_pair,
    match_borrowed_source_segment,
};

#[cfg(test)]
use super::BORROWED_NODE_VISITS;
use super::strings::{with_next_quoted_text, with_quoted_text};
use super::{
    BorrowedEvent, BorrowedScanQuery, BorrowedScanResult, BorrowedScanScratch, ScalarWalker,
    json_whitespace,
};

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

fn may_contain_selected_text(
    path: &[Cow<'_, str>],
    query: &BorrowedScanQuery<'_>,
    suppress_filter: bool,
) -> bool {
    (!suppress_filter
        && query.filter.is_some_and(|term| {
            path_is(path, &term.path.segments) || path_prefix(path, &term.path.segments)
        }))
        || query.fts.is_some_and(|fts| {
            fts.fields.is_empty()
                || fts.fields.iter().any(|field| {
                    fts_field_selects_path(&field.path.segments, path)
                        || path_prefix(path, &field.path.segments)
                })
        })
}

impl<'a> ScalarWalker<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn scan_borrowed_value(
        &mut self,
        depth: usize,
        path: &mut SmallVec<[Cow<'a, str>; 8]>,
        decoded: &mut String,
        events: &mut Vec<BorrowedEvent>,
        query: &BorrowedScanQuery<'_>,
        suppress_filter: bool,
        filter_element: bool,
    ) -> Result<(), String> {
        #[cfg(test)]
        BORROWED_NODE_VISITS.with(|count| count.set(count.get() + 1));
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
                            .any(|field| fts_field_selects_path(&field.path.segments, path))
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
                    if let (Some(fts), true) = (fts_here, selected_filter) {
                        (event.fts_hit, event.filter_hit) = match_borrowed_source_pair(
                            fts,
                            path,
                            text,
                            query.filter.expect("selected filter"),
                            &mut event.evidence,
                            query.retain_evidence,
                        )?;
                    } else if let Some(fts) = fts_here {
                        event.fts_hit = match_borrowed_source_segment(
                            fts,
                            path,
                            text,
                            &mut event.evidence,
                            query.retain_evidence,
                        )?;
                    } else if selected_filter {
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
                let root_field_fts = query.fts.is_some_and(|fts| {
                    fts.fields
                        .iter()
                        .any(|field| field.path.segments.is_empty())
                });
                let resolve_here = unscoped_value_fts
                    || root_field_fts
                    || (!suppress_filter
                        && query
                            .filter
                            .is_some_and(|term| path_prefix(path, &term.path.segments)))
                    || query.fts.is_some_and(|fts| {
                        fts.fields
                            .iter()
                            .any(|field| path_prefix(path, &field.path.segments))
                    });
                // With a lone FILTER term only one child key can contribute at
                // this object depth. Retain its previous event range directly,
                // avoiding an owned key and tree node for every source record.
                let single_filter_child = query.fts.is_none()
                    && !suppress_filter
                    && query
                        .filter
                        .is_some_and(|term| path_prefix(path, &term.path.segments));
                let mut previous_filter = None;
                let mut previous = (resolve_here && !single_filter_child)
                    .then(BTreeMap::<String, Range<usize>>::new);
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
                    let selected_child = single_filter_child
                        && query
                            .filter
                            .is_some_and(|term| term.path.segments[path.len()] == key.as_ref());
                    if single_filter_child && !selected_child {
                        self.skip_trusted_value(depth + 1)?;
                    } else {
                        let start = events.len();
                        path.push(key);
                        let irrelevant_container =
                            query.fts.is_none_or(|fts| !fts.fields.is_empty())
                                && matches!(self.bytes.get(self.pos), Some(b'{' | b'['))
                                && !may_contain_selected_text(
                                    path,
                                    query,
                                    suppress_filter || selected_filter,
                                );
                        if !irrelevant_container {
                            self.scan_borrowed_value(
                                depth + 1,
                                path,
                                decoded,
                                events,
                                query,
                                suppress_filter || selected_filter,
                                false,
                            )?;
                        } else {
                            self.skip_trusted_value(depth + 1)?;
                        }
                        let key = path.pop().expect("object key");
                        let track_key = selected_child
                            || unscoped_value_fts
                            || root_field_fts
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
                        if track_key {
                            let replaced = if single_filter_child {
                                previous_filter.replace(start..events.len())
                            } else {
                                previous.as_mut().and_then(|previous| {
                                    previous.insert(key.into_owned(), start..events.len())
                                })
                            };
                            if let Some(range) = replaced {
                                for event in &mut events[range] {
                                    event.visible = false;
                                }
                            }
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
