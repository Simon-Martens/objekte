//! The open record: a cursor over its field rows, collapsed branches, the
//! raw JSON view, find, and the actions on the selected field. Laid-out
//! rows are cached per record and width, so drawing never re-walks a large
//! record on every frame.

use super::app::{App, Hit, Line};
use super::detail::{self, DetailColumns, DetailRow};
use super::input::Input;
use regex::Regex;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
};

/// Cached layouts: the preview and the record view, plus one spare.
const CACHED: usize = 3;


/// Rows of one record laid out for one width.
pub(super) struct Layout {
    /// Shown rows; collapsed branches hide their descendants.
    pub(super) rows: Vec<DetailRow>,
    pub(super) columns: DetailColumns,
    /// Hidden descendants per shown row, for collapsed branches.
    pub(super) hidden: Vec<usize>,
    /// First line of the first row the search matched.
    pub(super) first_match: Option<usize>,
}


impl Layout {
    /// Line range of a shown row.
    pub(super) fn lines_of(&self, row: usize) -> std::ops::Range<usize> {
        let start = self.columns.row.partition_point(|&line_row| line_row < row);
        let end = self
            .columns
            .row
            .partition_point(|&line_row| line_row <= row);
        start..end
    }
}


#[derive(Clone, PartialEq, Eq)]
struct LayoutKey {
    collection: String,
    id: String,
    hash: String,
    metadata: bool,
    width: usize,
    collapsed: Option<u64>,
}


/// Collection, ID, and content hash of a record version.
type RecordKey = (String, String, String);


#[derive(Default)]
pub(super) struct RecordState {
    /// Selected shown row in the fields view.
    pub(super) cursor: usize,
    /// First line shown; drawing keeps the cursor inside the view.
    pub(super) scroll: Cell<usize>,
    /// Body height from the last frame.
    pub(super) height: Cell<usize>,
    /// Branches the user folded, and arrays the user unfolded; arrays
    /// start folded. Both are kept when stepping to the next record.
    pub(super) collapsed: BTreeSet<String>,
    pub(super) expanded: BTreeSet<String>,
    collapsed_version: u64,
    /// Pretty JSON instead of field rows.
    pub(super) raw: bool,
    /// The find line while it is being typed.
    pub(super) find_input: Option<Input>,
    /// Confirmed find text.
    pub(super) find: String,
    cache: RefCell<Vec<(LayoutKey, Rc<Layout>)>>,
    raw_cache: RefCell<Option<(RecordKey, Rc<Vec<String>>)>>,
}


impl RecordState {
    /// A new record: back to the top, keeping collapsed branches.
    pub(super) fn reset(&mut self) {
        self.cursor = 0;
        self.scroll.set(0);
        self.find_input = None;
    }

    /// Fold or unfold one branch.
    pub(super) fn set_folded(&mut self, pointer: &str, folded: bool) {
        if folded {
            self.expanded.remove(pointer);
            self.collapsed.insert(pointer.to_owned());
        } else {
            self.collapsed.remove(pointer);
            self.expanded.insert(pointer.to_owned());
        }
        self.collapsed_version += 1;
    }

    /// Whether a branch is folded: by the user, or as an array nobody
    /// opened that holds no search match.
    fn folded(&self, row: &DetailRow, matched: &[String]) -> bool {
        row.branch
            && (self.collapsed.contains(&row.pointer)
                || row.array
                    && !self.expanded.contains(&row.pointer)
                    && !matched.iter().any(|pointer| {
                        pointer.len() > row.pointer.len()
                            && pointer.starts_with(&row.pointer)
                            && pointer.as_bytes()[row.pointer.len()] == b'/'
                    }))
    }

    /// The find pattern: literal and case-insensitive.
    pub(super) fn find_regex(&self) -> Option<Regex> {
        let text = self
            .find_input
            .as_ref()
            .map_or(self.find.as_str(), |input| input.text.as_str());
        (!text.is_empty())
            .then(|| Regex::new(&format!("(?i){}", regex::escape(text))).ok())
            .flatten()
    }

    /// Layout for `hit`, from the cache when nothing changed.
    pub(super) fn layout(
        &self,
        hit: &Hit,
        metadata: bool,
        width: usize,
        collapse: bool,
    ) -> Rc<Layout> {
        let record = hit.record();
        let key = LayoutKey {
            collection: record.collection.clone(),
            id: record.id.clone(),
            hash: record.hash.clone(),
            metadata,
            width,
            collapsed: collapse.then_some(self.collapsed_version),
        };
        if let Some((_, layout)) = self.cache.borrow().iter().find(|(old, _)| *old == key) {
            return Rc::clone(layout);
        }
        let all = detail::rows(hit, metadata);
        let matched = all
            .iter()
            .filter(|row| row.matched)
            .map(|row| row.pointer.clone())
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        let mut hidden = Vec::new();
        let mut hiding: Option<String> = None;
        for row in all {
            if let Some(pointer) = &hiding {
                if row.is_inside(pointer) {
                    *hidden.last_mut().expect("collapsed branch") += 1;
                    continue;
                }
                hiding = None;
            }
            if collapse && self.folded(&row, &matched) {
                hiding = Some(row.pointer.clone());
            }
            rows.push(row);
            hidden.push(0);
        }
        let shown = rows.iter().collect::<Vec<_>>();
        let columns = detail::columns(&shown, (width * 2 / 5).max(10), width);
        let first_match = columns.matched.iter().position(|matched| *matched);
        let layout = Rc::new(Layout {
            rows,
            columns,
            hidden,
            first_match,
        });
        let mut cache = self.cache.borrow_mut();
        cache.retain(|(old, _)| *old != key);
        cache.insert(0, (key, Rc::clone(&layout)));
        cache.truncate(CACHED);
        layout
    }

    /// The record as pretty JSON lines, or its raw text when unparsable.
    pub(super) fn raw_lines(&self, hit: &Hit) -> Rc<Vec<String>> {
        let record = hit.record();
        let key = (
            record.collection.clone(),
            record.id.clone(),
            record.hash.clone(),
        );
        if let Some((old, lines)) = &*self.raw_cache.borrow()
            && *old == key
        {
            return Rc::clone(lines);
        }
        let text = match &record.raw {
            Some(raw) => String::from_utf8_lossy(raw).into_owned(),
            None => serde_json::to_string_pretty(&record.data).unwrap_or_default(),
        };
        let lines = Rc::new(text.lines().map(str::to_owned).collect::<Vec<_>>());
        *self.raw_cache.borrow_mut() = Some((key, Rc::clone(&lines)));
        lines
    }
}


impl App<'_> {
    /// Layout of the open record at the width of the last frame.
    pub(super) fn record_layout(&self) -> Option<Rc<Layout>> {
        let hit = self.current_detail_hit()?;
        Some(self.record.layout(hit, true, self.record_width(), true))
    }

    pub(super) fn record_width(&self) -> usize {
        self.record_width.get().max(20)
    }

    /// The row under the cursor.
    pub(super) fn record_row(&self) -> Option<(Rc<Layout>, usize)> {
        let layout = self.record_layout()?;
        let cursor = self.record.cursor.min(layout.rows.len().checked_sub(1)?);
        Some((layout, cursor))
    }

    /// Move the cursor by rows, or scroll the raw view by lines.
    pub(super) fn record_move(&mut self, delta: isize) {
        if self.record.raw {
            let lines = self
                .current_detail_hit()
                .map_or(0, |hit| self.record.raw_lines(hit).len());
            let height = self.record.height.get().max(1);
            let scroll = self
                .record
                .scroll
                .get()
                .saturating_add_signed(delta)
                .min(lines.saturating_sub(height));
            self.record.scroll.set(scroll);
            return;
        }
        let Some(layout) = self.record_layout() else {
            return;
        };
        let last = layout.rows.len().saturating_sub(1);
        let mut cursor = self.record.cursor.saturating_add_signed(delta).min(last);
        // Separators are not selectable.
        while layout.rows.get(cursor).is_some_and(DetailRow::is_separator) {
            cursor = if delta < 0 {
                cursor.saturating_sub(1)
            } else {
                (cursor + 1).min(last)
            };
            if cursor == 0 || cursor == last {
                break;
            }
        }
        self.record.cursor = cursor;
    }

    pub(super) fn record_home(&mut self, end: bool) {
        if end {
            self.record_move(isize::MAX / 2);
        } else {
            self.record.cursor = 0;
            self.record.scroll.set(0);
        }
    }

    /// Enter on a field: follow its link, or fold and unfold a branch.
    pub(super) fn record_enter(&mut self) {
        let Some((layout, cursor)) = self.record_row() else {
            return;
        };
        let row = &layout.rows[cursor];
        if let Some(index) = self
            .links
            .outgoing
            .iter()
            .position(|edge| !row.pointer.is_empty() && edge.location == row.pointer)
        {
            self.links.outgoing_selected = index;
            self.links.section = super::navigation::DetailSection::Links;
            self.open_selected_relation();
            return;
        }
        if row.branch {
            let pointer = row.pointer.clone();
            let folded = layout.hidden[cursor] > 0;
            self.record.set_folded(&pointer, !folded);
        }
    }

    /// Space: fold or unfold the branch under the cursor, or its parent.
    pub(super) fn record_toggle(&mut self) {
        let Some((layout, cursor)) = self.record_row() else {
            return;
        };
        let row = &layout.rows[cursor];
        if row.branch {
            let pointer = row.pointer.clone();
            let folded = layout.hidden[cursor] > 0;
            self.record.set_folded(&pointer, !folded);
            return;
        }
        // On a leaf, fold the nearest enclosing branch and select it.
        if let Some(parent) = (0..cursor)
            .rev()
            .find(|&index| layout.rows[index].branch && row.is_inside(&layout.rows[index].pointer))
        {
            let pointer = layout.rows[parent].pointer.clone();
            self.record.set_folded(&pointer, true);
            self.record.cursor = parent;
        }
    }

    /// Add `field = value` for the selected field to the filter.
    pub(super) fn record_filter_by_value(&mut self) {
        let Some((layout, cursor)) = self.record_row() else {
            return;
        };
        let row = &layout.rows[cursor];
        let Some(value) = &row.scalar else {
            self.notice("Choose a field with a single value to filter by it");
            return;
        };
        if row.filter_path.is_empty() {
            self.notice("This field cannot be filtered");
            return;
        }
        let literal = match value {
            Value::String(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
            other => other.to_string(),
        };
        // A field inside an array reaches several values.
        let operator = if row.many { "?=" } else { "=" };
        let term = format!("{} {operator} {literal}", row.filter_path);
        // Keep a trailing sort clause at the end.
        let current = self.filter.text.clone();
        let (filter, sort) = match current.to_ascii_uppercase().rfind("SORT BY") {
            Some(start) => (
                current[..start].to_owned(),
                Some(current[start..].trim().to_owned()),
            ),
            None => (current, None),
        };
        let mut text = if filter.trim().is_empty() {
            term
        } else {
            format!("{} && {term}", filter.trim_end())
        };
        if let Some(clause) = sort {
            text = format!("{text} {clause}");
        }
        self.filter = Input::with_text(&text);
        self.line = Line::Filter;
        self.close_detail();
        self.search();
    }

    /// Count the values of the selected field over the whole result.
    pub(super) fn record_breakdown(&mut self) {
        let Some((layout, cursor)) = self.record_row() else {
            return;
        };
        let path = layout.rows[cursor].filter_path.clone();
        if path.is_empty() || path.starts_with('$') {
            self.notice("Choose a field to count its values");
            return;
        }
        if self.links.override_hit.is_some() || self.last_request.is_none() {
            self.notice("Counting needs a search result");
            return;
        }
        self.defer(super::lists::Deferred::Breakdown(path));
        self.notice("Counting values…");
    }

    /// Text of the selected field: its value, or the JSON of a branch.
    pub(super) fn record_copy_text(&self, whole: bool) -> Option<String> {
        let hit = self.current_detail_hit()?;
        let record = hit.record();
        if whole {
            return Some(match &record.raw {
                Some(raw) => String::from_utf8_lossy(raw).into_owned(),
                None => serde_json::to_string_pretty(&record.data).ok()?,
            });
        }
        let (layout, cursor) = self.record_row()?;
        let row = &layout.rows[cursor];
        if row.branch {
            let value = record.data.pointer(&row.pointer)?;
            return serde_json::to_string_pretty(value).ok();
        }
        Some(match &row.scalar {
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => row.value.clone(),
        })
    }

    /// Jump to the next or previous row (or raw line) matching find.
    pub(super) fn record_find_next(&mut self, forward: bool) {
        let Some(regex) = self.record.find_regex() else {
            self.notice("Press / to find text in this record");
            return;
        };
        if self.record.raw {
            let Some(hit) = self.current_detail_hit() else {
                return;
            };
            let lines = self.record.raw_lines(hit);
            let start = self.record.scroll.get();
            let found = if forward {
                (start + 1..lines.len())
                    .chain(0..=start.min(lines.len().saturating_sub(1)))
                    .find(|&line| regex.is_match(&lines[line]))
            } else {
                (0..start)
                    .rev()
                    .chain((start..lines.len()).rev())
                    .find(|&line| regex.is_match(&lines[line]))
            };
            match found {
                Some(line) => self.record.scroll.set(line),
                None => self.notice("No match in this record"),
            }
            return;
        }
        // Search every row, folded or not, then unfold to show the match.
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let all = self.record.layout(hit, true, self.record_width(), false);
        let Some(shown) = self.record_layout() else {
            return;
        };
        let count = all.rows.len();
        let matches = |index: usize| {
            let row = &all.rows[index];
            regex.is_match(&row.name) || regex.is_match(&row.value)
        };
        // Where the cursor is among all rows.
        let at = shown
            .rows
            .get(self.record.cursor)
            .and_then(|current| {
                all.rows
                    .iter()
                    .position(|row| row.pointer == current.pointer && row.name == current.name)
            })
            .unwrap_or(0);
        let found = if forward {
            (at + 1..count).chain(0..=at).find(|&index| matches(index))
        } else {
            (0..at)
                .rev()
                .chain((at..count).rev())
                .find(|&index| matches(index))
        };
        let Some(index) = found else {
            self.notice("No match in this record");
            return;
        };
        let target = &all.rows[index];
        // Open every branch on the way to the match.
        let ancestors = all
            .rows
            .iter()
            .filter(|row| row.branch && target.is_inside(&row.pointer))
            .map(|row| row.pointer.clone())
            .collect::<Vec<_>>();
        for pointer in ancestors {
            if self.record.collapsed.contains(&pointer) || !self.record.expanded.contains(&pointer)
            {
                self.record.set_folded(&pointer, false);
            }
        }
        let (pointer, name) = (target.pointer.clone(), target.name.clone());
        if let Some(layout) = self.record_layout()
            && let Some(position) = layout
                .rows
                .iter()
                .position(|row| row.pointer == pointer && row.name == name)
        {
            self.record.cursor = position;
        }
    }

    /// Matches of the confirmed find text, for the footer.
    pub(super) fn record_find_count(&self) -> usize {
        let Some(regex) = self.record.find_regex() else {
            return 0;
        };
        if self.record.raw {
            return self.current_detail_hit().map_or(0, |hit| {
                self.record
                    .raw_lines(hit)
                    .iter()
                    .filter(|line| regex.is_match(line))
                    .count()
            });
        }
        self.current_detail_hit().map_or(0, |hit| {
            self.record
                .layout(hit, true, self.record_width(), false)
                .rows
                .iter()
                .filter(|row| regex.is_match(&row.name) || regex.is_match(&row.value))
                .count()
        })
    }
}


#[cfg(test)]
#[path = "record_view.test.rs"]
mod tests;
