//! List dialogs: collections, actions, history, saved queries, sorting,
//! columns, and value breakdowns. Each list is filtered by typing; Enter
//! takes the selected entry.

use super::app::{Action, App, Line, Overlay, Request};
use super::input::Input;
use super::settings::{Query, SavedQuery};
use objekte_core::{JsonFilter, JsonPathStep, Record, RecordStatus};
use serde_json::Value;
use std::collections::BTreeMap;


#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ListKind {
    Collections,
    Palette,
    History,
    Saved,
    SortBy,
    Columns,
    /// Choose the field to count values of.
    BreakdownField,
    /// Value counts of the chosen field.
    Breakdown,
}


impl ListKind {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Collections => "Search in",
            Self::Palette => "Actions",
            Self::History => "Recent queries",
            Self::Saved => "Saved queries",
            Self::SortBy => "Sort by",
            Self::Columns => "Columns",
            Self::BreakdownField => "Count values of",
            Self::Breakdown => "Values",
        }
    }

    pub(super) fn placeholder(self) -> &'static str {
        match self {
            Self::Palette => "type a command",
            _ => "type to filter",
        }
    }

    /// Keys shown at the bottom of the dialog.
    pub(super) fn keys(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::History | Self::Saved => &[
                ("↑↓", "choose"),
                ("Enter", "search"),
                ("Del", "remove"),
                ("Esc", "close"),
            ],
            Self::Columns => &[("↑↓", "choose"), ("Space", "show / hide"), ("Esc", "close")],
            Self::Breakdown => &[
                ("↑↓", "choose"),
                ("Enter", "filter by value"),
                ("Esc", "close"),
            ],
            _ => &[("↑↓", "choose"), ("Enter", "select"), ("Esc", "close")],
        }
    }
}


#[derive(Clone, PartialEq, Debug)]
pub(super) enum ListValue {
    Scope(usize),
    Action(Action),
    History(usize),
    Saved(usize),
    Sort(String, bool),
    Column(String),
    Field(String),
    /// A counted value; none counts records without the field.
    Count(Option<Value>),
}


pub(super) struct ListItem {
    pub(super) label: String,
    pub(super) note: String,
    pub(super) value: ListValue,
    /// Shown as a check box when set.
    pub(super) checked: Option<bool>,
    /// Share of the largest count, for a small bar.
    pub(super) bar: Option<f64>,
}


impl ListItem {
    fn new(label: impl Into<String>, note: impl Into<String>, value: ListValue) -> Self {
        Self {
            label: label.into(),
            note: note.into(),
            value,
            checked: None,
            bar: None,
        }
    }
}


/// Value counts of one field over the whole result.
pub(super) struct Breakdown {
    pub(super) field: String,
    /// Some record had several values, so filters need `?=`.
    pub(super) many: bool,
    pub(super) counts: Vec<(Option<Value>, usize)>,
    pub(super) records: usize,
    pub(super) truncated: bool,
}


/// Work that needs the repository, run once no worker has borrowed it.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Deferred {
    Edit(std::path::PathBuf),
    Refresh,
    Breakdown(String),
    CollectionCounts,
    Export,
}


fn matches(label: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let label = label.to_lowercase();
    let filter = filter.to_lowercase();
    if label.contains(&filter) {
        return true;
    }
    let mut chars = label.chars();
    filter.chars().all(|wanted| chars.any(|ch| ch == wanted))
}

fn describe(query: &Query) -> String {
    let mut parts = Vec::new();
    if !query.search.is_empty() {
        parts.push(query.search.clone());
    }
    if !query.filter.is_empty() {
        parts.push(format!("[{}]", query.filter));
    }
    parts.join("  ")
}

fn literal(value: &Value) -> String {
    match value {
        Value::String(text) => format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'")),
        other => other.to_string(),
    }
}

fn value_label(value: &Option<Value>) -> String {
    match value {
        None => "(missing)".into(),
        Some(Value::String(text)) if text.is_empty() => "(empty)".into(),
        Some(Value::String(text)) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Some(other) => other.to_string(),
    }
}

/// The distinct scalar values a path reaches in a record. Arrays are
/// traversed implicitly, as in filters.
fn values_at(record: &Record, path: &[JsonPathStep]) -> Vec<Value> {
    fn walk(value: &Value, path: &[JsonPathStep], out: &mut Vec<Value>) {
        if let Value::Array(items) = value
            && !matches!(
                path.first(),
                Some(JsonPathStep::Index(_) | JsonPathStep::Each)
            )
        {
            for item in items {
                walk(item, path, out);
            }
            return;
        }
        let Some((step, rest)) = path.split_first() else {
            if !value.is_null() && !out.contains(value) {
                out.push(value.clone());
            }
            return;
        };
        match (step, value) {
            (JsonPathStep::Key(name), Value::Object(map)) => {
                if let Some(child) = map.get(name) {
                    walk(child, rest, out);
                }
            }
            (JsonPathStep::Index(index), Value::Array(items)) => {
                if let Some(child) = items.get(*index) {
                    walk(child, rest, out);
                }
            }
            (JsonPathStep::Each, Value::Array(items)) => {
                for item in items {
                    walk(item, rest, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(&record.data, path, &mut out);
    out
}

fn status_value(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Valid => "valid",
        RecordStatus::Invalid => "invalid",
        RecordStatus::IncorrectJson => "incorrect_json",
        RecordStatus::IncorrectFormat => "incorrect_format",
        RecordStatus::NoSchema => "no_schema",
        RecordStatus::Pending => "pending",
    }
}


impl App<'_> {
    pub(super) fn open_list(&mut self, kind: ListKind) {
        self.completion = None;
        let selected = if kind == ListKind::Collections {
            self.scope
        } else {
            0
        };
        self.overlay = Some(Overlay::List(
            kind,
            super::app::Chooser {
                selected,
                ..Default::default()
            },
        ));
        if kind == ListKind::Collections && self.last_request.is_some() {
            self.defer(Deferred::CollectionCounts);
        }
    }

    pub(super) fn defer(&mut self, job: Deferred) {
        if !self.deferred.contains(&job) {
            self.deferred.push(job);
        }
    }

    /// Run deferred work now that the repository is free. An edit is
    /// returned for the event loop, which has to hand over the terminal.
    pub(super) fn run_deferred(&mut self) -> Option<std::path::PathBuf> {
        let mut edit = None;
        for job in std::mem::take(&mut self.deferred) {
            if self.indexes_busy() {
                self.notice("Wait until the indexes are ready");
                continue;
            }
            match job {
                Deferred::Edit(path) => edit = Some(path),
                Deferred::Refresh => self.refresh_files(),
                Deferred::Breakdown(field) => self.run_breakdown(&field),
                Deferred::CollectionCounts => self.run_collection_counts(),
                Deferred::Export => self.finish_export(),
            }
        }
        edit
    }

    /// The entries of a list, filtered by `filter`.
    pub(super) fn list_items(&self, kind: ListKind, filter: &str) -> Vec<ListItem> {
        let mut items: Vec<ListItem> = match kind {
            ListKind::Collections => (0..=self.collections.len())
                .map(|scope| {
                    let name = if scope == 0 {
                        "all collections"
                    } else {
                        self.collections[scope - 1].as_str()
                    };
                    let count = self.collection_counts.as_ref().map(|(counts, truncated)| {
                        let count = if scope == 0 {
                            counts.values().sum()
                        } else {
                            counts.get(name).copied().unwrap_or(0)
                        };
                        format!("{count}{}", if *truncated { "+" } else { "" })
                    });
                    let current = (scope == self.scope).then_some("current");
                    let note = [current.map(str::to_owned), count]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ListItem::new(name, note, ListValue::Scope(scope))
                })
                .collect(),
            ListKind::Palette => Action::ALL
                .into_iter()
                .map(|action| {
                    ListItem::new(action.label(), action.shortcut(), ListValue::Action(action))
                })
                .collect(),
            ListKind::History => self
                .settings
                .history
                .iter()
                .enumerate()
                .rev()
                .map(|(index, query)| {
                    ListItem::new(
                        describe(query),
                        query.collection.clone().unwrap_or_default(),
                        ListValue::History(index),
                    )
                })
                .collect(),
            ListKind::Saved => self
                .settings
                .saved
                .iter()
                .enumerate()
                .map(|(index, saved)| {
                    ListItem::new(
                        saved.name.clone(),
                        describe(&saved.query),
                        ListValue::Saved(index),
                    )
                })
                .collect(),
            ListKind::SortBy => {
                let mut keys = self
                    .columns
                    .iter()
                    .map(|column| column.key.clone())
                    .filter(|key| !matches!(key.as_str(), "@match" | "@status" | "$value"))
                    .collect::<Vec<_>>();
                for system in ["@modified", "@size"] {
                    if !keys.iter().any(|key| key == system) {
                        keys.push(system.into());
                    }
                }
                keys.into_iter()
                    .flat_map(|key| {
                        [
                            ListItem::new(
                                format!("{key} ↑"),
                                "ascending",
                                ListValue::Sort(key.clone(), false),
                            ),
                            ListItem::new(
                                format!("{key} ↓"),
                                "descending",
                                ListValue::Sort(key, true),
                            ),
                        ]
                    })
                    .collect()
            }
            ListKind::Columns => {
                let hidden = self.settings.hidden(self.scope_name());
                self.columns
                    .iter()
                    .filter(|column| column.key != "@id")
                    .map(|column| {
                        let mut item = ListItem::new(
                            column.key.trim_start_matches('@'),
                            "",
                            ListValue::Column(column.key.clone()),
                        );
                        item.checked = Some(!hidden.contains(&column.key));
                        item
                    })
                    .collect()
            }
            ListKind::BreakdownField => {
                let mut keys = vec!["@collection".to_string(), "@status".to_string()];
                keys.extend(
                    self.columns
                        .iter()
                        .map(|column| column.key.clone())
                        .filter(|key| !key.starts_with('@') && key != "$value"),
                );
                keys.into_iter()
                    .map(|key| ListItem::new(key.clone(), "", ListValue::Field(key)))
                    .collect()
            }
            ListKind::Breakdown => {
                let Some(breakdown) = &self.breakdown else {
                    return Vec::new();
                };
                let largest = breakdown
                    .counts
                    .first()
                    .map_or(1, |(_, count)| *count)
                    .max(1);
                breakdown
                    .counts
                    .iter()
                    .map(|(value, count)| {
                        let mut item = ListItem::new(
                            value_label(value),
                            count.to_string(),
                            ListValue::Count(value.clone()),
                        );
                        item.bar = Some(*count as f64 / largest as f64);
                        item
                    })
                    .collect()
            }
        };
        items.retain(|item| matches(&item.label, filter));
        items
    }

    /// Enter on a list entry.
    pub(super) fn list_accept(&mut self, kind: ListKind, value: ListValue) {
        match value {
            ListValue::Scope(scope) => {
                self.overlay = None;
                self.set_scope(scope);
            }
            ListValue::Action(action) => self.run_action(action),
            ListValue::History(index) => {
                self.overlay = None;
                if let Some(query) = self.settings.history.get(index).cloned() {
                    self.restore_query(&query);
                }
            }
            ListValue::Saved(index) => {
                self.overlay = None;
                if let Some(saved) = self.settings.saved.get(index).cloned() {
                    self.restore_query(&saved.query);
                }
            }
            ListValue::Sort(key, descending) => {
                self.overlay = None;
                self.set_sort(&key, descending);
            }
            ListValue::Column(key) => {
                let scope = self.scope_name().map(str::to_owned);
                self.settings.toggle_column(scope.as_deref(), &key);
                self.save_settings();
            }
            ListValue::Field(field) => {
                self.overlay = None;
                self.defer(Deferred::Breakdown(field));
                self.notice("Counting values…");
            }
            ListValue::Count(value) => {
                self.overlay = None;
                self.filter_by_count(value);
            }
        }
        let _ = kind;
    }

    /// Delete on a history or saved entry.
    pub(super) fn list_remove(&mut self, value: &ListValue) {
        match value {
            ListValue::History(index) if *index < self.settings.history.len() => {
                self.settings.history.remove(*index);
            }
            ListValue::Saved(index) if *index < self.settings.saved.len() => {
                let saved = self.settings.saved.remove(*index);
                self.notice(format!("Removed {}", saved.name));
            }
            _ => return,
        }
        self.save_settings();
    }

    /// Save the current query under a name, replacing one with that name.
    pub(super) fn save_query(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.notice("Give the query a name");
            return;
        }
        let query = self.current_query();
        self.settings.saved.retain(|saved| saved.name != name);
        self.settings.saved.push(SavedQuery {
            name: name.to_owned(),
            query,
        });
        self.settings
            .saved
            .sort_by_key(|saved| saved.name.to_lowercase());
        self.save_settings();
        self.overlay = None;
        self.notice(format!("Saved as {name} · Ctrl+B lists saved queries"));
    }

    pub(super) fn open_save_query(&mut self) {
        let query = self.current_query();
        if query.search.trim().is_empty() && query.filter.trim().is_empty() {
            self.notice("Type a query to save it");
            return;
        }
        let name = describe(&query).chars().take(40).collect::<String>();
        self.overlay = Some(Overlay::SaveQuery(Input::with_text(&name)));
    }

    /// Replace the sort clause, keeping it on the line that had one.
    pub(super) fn set_sort(&mut self, key: &str, descending: bool) {
        let clause = format!("SORT BY {key}{}", if descending { " DESC" } else { "" });
        let strip = |text: &str| match text.to_ascii_uppercase().rfind("SORT BY") {
            Some(start) => text[..start].trim_end().to_owned(),
            None => text.trim_end().to_owned(),
        };
        let on_search = self.search.text.to_ascii_uppercase().contains("SORT BY");
        let line = if on_search {
            &mut self.search
        } else {
            &mut self.filter
        };
        let kept = strip(&line.text);
        let text = if kept.is_empty() {
            clause
        } else {
            format!("{kept} {clause}")
        };
        *line = Input::with_text(&text);
        self.search();
    }

    fn filter_by_count(&mut self, value: Option<Value>) {
        let Some(breakdown) = self.breakdown.take() else {
            return;
        };
        if self.view == super::app::View::Detail {
            self.close_detail();
        }
        let field = breakdown.field.clone();
        if field == "@collection" {
            if let Some(Value::String(name)) = &value
                && let Some(index) = self.collections.iter().position(|known| known == name)
            {
                self.set_scope(index + 1);
            }
            return;
        }
        let term = match &value {
            None => format!("missing({field})"),
            Some(value) if breakdown.many => format!("{field} ?= {}", literal(value)),
            Some(value) => format!("{field} = {}", literal(value)),
        };
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
        self.search();
    }

    /// The shown query over the whole result, optionally in every collection.
    fn whole_request(&self, every_collection: bool) -> Option<Request> {
        let mut request = self.last_request.as_ref()?.whole_result();
        if every_collection {
            match &mut request {
                Request::Filter(query) => query.collections = None,
                Request::Text(query) => query.collections = None,
            }
        }
        Some(request)
    }

    /// Visit every record of a whole-result request.
    fn each_record(
        &mut self,
        request: &Request,
        mut visit: impl FnMut(&Record),
    ) -> Result<bool, String> {
        let truncated = match request {
            Request::Filter(query) => {
                self.repo
                    .query_json_stream(query, |_, record| {
                        visit(record);
                        Ok(())
                    })
                    .map_err(|error| error.to_string())?
                    .truncated
            }
            Request::Text(query) => {
                self.repo
                    .search_text_stream(query, |_, hit| {
                        visit(&hit.record);
                        Ok(())
                    })
                    .map_err(|error| error.to_string())?
                    .truncated
            }
        };
        Ok(truncated)
    }

    /// Count the values of `field` over the whole result.
    pub(super) fn run_breakdown(&mut self, field: &str) {
        let Some(request) = self.whole_request(false) else {
            self.notice("Search first, then count values");
            return;
        };
        let path = if field.starts_with('@') {
            None
        } else {
            match JsonFilter::parse_path(field) {
                Ok(path) => Some(path),
                Err(error) => {
                    self.error(format!("Cannot count {field}: {error}"));
                    return;
                }
            }
        };
        let mut counts = BTreeMap::<String, (Option<Value>, usize)>::new();
        let mut records = 0;
        let mut many = false;
        let result = self.each_record(&request, |record| {
            records += 1;
            let values = match (field, &path) {
                ("@collection", _) => vec![Value::String(record.collection.clone())],
                ("@status", _) => vec![Value::String(status_value(record.state.status).into())],
                (_, Some(path)) => values_at(record, path),
                _ => Vec::new(),
            };
            many |= values.len() > 1;
            if values.is_empty() {
                counts.entry(String::new()).or_insert((None, 0)).1 += 1;
            }
            for value in values {
                counts
                    .entry(format!("v{value}"))
                    .or_insert_with(|| (Some(value.clone()), 0))
                    .1 += 1;
            }
        });
        match result {
            Ok(truncated) => {
                let mut counts = counts.into_values().collect::<Vec<_>>();
                counts.sort_by(|a, b| {
                    b.1.cmp(&a.1)
                        .then_with(|| value_label(&a.0).cmp(&value_label(&b.0)))
                });
                self.breakdown = Some(Breakdown {
                    field: field.to_owned(),
                    many,
                    counts,
                    records,
                    truncated,
                });
                self.notice = None;
                self.overlay = Some(Overlay::List(ListKind::Breakdown, Default::default()));
            }
            Err(error) => self.error(format!("Cannot count values: {error}")),
        }
    }

    /// Matches per collection for the collection picker.
    pub(super) fn run_collection_counts(&mut self) {
        let Some(request) = self.whole_request(true) else {
            return;
        };
        let mut counts = BTreeMap::<String, usize>::new();
        match self.each_record(&request, |record| {
            *counts.entry(record.collection.clone()).or_default() += 1;
        }) {
            Ok(truncated) => self.collection_counts = Some((counts, truncated)),
            Err(error) => self.error(format!("Cannot count matches: {error}")),
        }
    }
}


#[cfg(test)]
#[path = "lists.test.rs"]
mod tests;
