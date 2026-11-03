//! Schema-driven completion for the JSON filter line.
//!
//! The engine looks at the token under the cursor and its context: a field
//! path, a value after a comparison, a function argument, or a sort term.
//! Field paths follow nested properties, array items, and `x-relation`
//! fields into the linked collection's schema.

use objekte_core::{Collection, Field, Record, ValueCount, ValueType};
use serde_json::Value;
use std::collections::BTreeMap;

/// Records sampled for fields and example values.
const SAMPLED: usize = 300;
static NO_FIELDS: BTreeMap<String, Field> = BTreeMap::new();


/// Fields and example values seen in loaded results. Collections without a
/// schema complete from these; schema fields show an example value.
#[derive(Default)]
pub(super) struct Seen {
    fields: BTreeMap<String, BTreeMap<String, Field>>,
    /// First value per collection and dotted path, such as `entry.year`.
    samples: BTreeMap<(String, String), String>,
}


impl Seen {
    pub(super) fn from_records<'r>(
        records: impl Iterator<Item = &'r Record>,
        schemas: &BTreeMap<String, Collection>,
    ) -> Self {
        let mut seen = Self::default();
        for record in records.take(SAMPLED) {
            let Value::Object(map) = &record.data else {
                continue;
            };
            let shapeless = schemas
                .get(&record.collection)
                .is_none_or(|collection| collection.fields.is_empty());
            for (name, value) in map {
                seen.sample(&record.collection, name, value, 0);
                if shapeless {
                    let fields = seen.fields.entry(record.collection.clone()).or_default();
                    merge(fields, name, value, 0);
                }
            }
        }
        seen
    }

    fn sample(&mut self, collection: &str, path: &str, value: &Value, depth: usize) {
        if depth > 8 {
            return;
        }
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    self.sample(collection, &format!("{path}.{name}"), child, depth + 1);
                }
            }
            Value::Array(items) => {
                if let Some(first) = items.iter().find(|item| !item.is_null()) {
                    self.sample(collection, path, first, depth + 1);
                }
            }
            Value::Null => {}
            Value::String(text) if text.trim().is_empty() => {}
            scalar => {
                self.samples
                    .entry((collection.to_owned(), path.to_owned()))
                    .or_insert_with(|| {
                        let text = match scalar {
                            Value::String(text) => {
                                text.split_whitespace().collect::<Vec<_>>().join(" ")
                            }
                            other => other.to_string(),
                        };
                        let mut short = text.chars().take(24).collect::<String>();
                        if text.chars().count() > 24 {
                            short.push('…');
                        }
                        short
                    });
            }
        }
    }
}


/// Add a seen value's shape to `fields`.
fn merge(fields: &mut BTreeMap<String, Field>, name: &str, value: &Value, depth: usize) {
    if value.is_null() || depth > 8 {
        return;
    }
    let field = fields
        .entry(name.to_owned())
        .or_insert_with(|| shape(value));
    match value {
        Value::Object(map) if field.kind == ValueType::Object => {
            for (child, child_value) in map {
                merge(&mut field.properties, child, child_value, depth + 1);
            }
        }
        Value::Array(items) if field.kind == ValueType::Array => {
            for item in items.iter().filter(|item| !item.is_null()) {
                let entry = field.items.get_or_insert_with(|| Box::new(shape(item)));
                if let (Value::Object(map), ValueType::Object) = (item, entry.kind) {
                    for (child, child_value) in map {
                        merge(&mut entry.properties, child, child_value, depth + 1);
                    }
                }
            }
        }
        _ => {}
    }
}

fn shape(value: &Value) -> Field {
    Field {
        kind: match value {
            Value::Object(_) => ValueType::Object,
            Value::Array(_) => ValueType::Array,
            Value::String(_) => ValueType::String,
            Value::Number(number) if number.is_i64() || number.is_u64() => ValueType::Integer,
            Value::Number(_) => ValueType::Number,
            Value::Bool(_) => ValueType::Boolean,
            Value::Null => ValueType::Null,
        },
        nullable: true,
        index: false,
        language: None,
        sort_index: false,
        sort_reduce: None,
        text_format: None,
        exclude_fts: false,
        enum_values: None,
        suggested_values: None,
        properties: BTreeMap::new(),
        items: None,
        relation: None,
    }
}


/// What kind of thing a candidate inserts; shown as a small icon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Field,
    Relation,
    System,
    Macro,
    Function,
    Keyword,
    Value,
    Operator,
}


/// One suggestion: the text it inserts and a short description.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Candidate {
    pub(super) insert: String,
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) kind: Kind,
}


/// Suggestions replacing characters `start..end` of the line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Completion {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) items: Vec<Candidate>,
}


const SYSTEM_FIELDS: [(&str, &str); 7] = [
    ("@id", "record ID"),
    ("@filename", "file name"),
    ("@path", "repository path"),
    ("@size", "file size, e.g. > 2KB"),
    ("@modified", "modification time"),
    ("@created", "creation time"),
    ("@status", "validation status"),
];

const MACROS: [(&str, &str); 16] = [
    ("@now", "current time"),
    ("@todayStart", "start of today (UTC)"),
    ("@todayEnd", "end of today (UTC)"),
    ("@yesterday", "24 hours ago"),
    ("@tomorrow", "in 24 hours"),
    ("@monthStart", "start of this month"),
    ("@monthEnd", "end of this month"),
    ("@yearStart", "start of this year"),
    ("@yearEnd", "end of this year"),
    ("@year", "current year"),
    ("@month", "current month, 1–12"),
    ("@day", "day of the month"),
    ("@weekday", "0 is Sunday"),
    ("@hour", "hour, 0–23"),
    ("@minute", "minute"),
    ("@second", "second"),
];

const OPERATORS: [(&str, &str); 12] = [
    ("=", "equals"),
    ("!=", "differs"),
    ("~", "contains / LIKE"),
    ("!~", "does not contain"),
    (">", "greater"),
    (">=", "greater or equal"),
    ("<", "less"),
    ("<=", "less or equal"),
    ("?=", "any value equals"),
    ("?!=", "any value differs"),
    ("?~", "any value contains"),
    ("?>", "any value greater"),
];

const MODIFIERS: [(&str, &str); 4] = [
    ("fts", "rendered full text"),
    ("lower", "lowercase"),
    ("length", "array length"),
    ("each", "every array value"),
];


/// Field schemas of the collections in scope, with access to every
/// collection for following relations.
/// Looks up the most common values of an indexed path in a collection.
pub(super) type Counter<'a> = &'a dyn Fn(&str, &str) -> Option<Vec<ValueCount>>;

/// Most common values offered after a comparison.
const TOP_VALUES: usize = 40;


pub(super) struct Schema<'a> {
    scope: Vec<&'a Collection>,
    all: &'a BTreeMap<String, Collection>,
    seen: Option<&'a Seen>,
    counts: Option<Counter<'a>>,
}


impl<'a> Schema<'a> {
    pub(super) fn new(all: &'a BTreeMap<String, Collection>, scope: Option<&str>) -> Self {
        Self {
            scope: all
                .values()
                .filter(|collection| scope.is_none_or(|name| collection.name == name))
                .collect(),
            all,
            seen: None,
            counts: None,
        }
    }

    /// Offer the most common indexed values after a comparison.
    pub(super) fn with_counts(mut self, counts: Counter<'a>) -> Self {
        self.counts = Some(counts);
        self
    }

    /// Indexed values of `path` across the scope, most records first.
    fn top_values(&self, path: &str) -> Vec<(Value, usize)> {
        let Some(counts) = self.counts else {
            return Vec::new();
        };
        let mut merged = Vec::<(Value, usize)>::new();
        for collection in &self.scope {
            for found in counts(&collection.name, path).into_iter().flatten() {
                match merged.iter_mut().find(|(value, _)| *value == found.value) {
                    Some((_, records)) => *records += found.records,
                    None => merged.push((found.value, found.records)),
                }
            }
        }
        merged.sort_by_key(|(_, records)| std::cmp::Reverse(*records));
        merged.truncate(TOP_VALUES);
        merged
    }

    /// Complete collections without a schema from fields seen in results.
    pub(super) fn with_seen(mut self, seen: &'a Seen) -> Self {
        self.seen = Some(seen);
        self
    }

    /// Declared fields, or the fields seen in results without a schema.
    fn fields_of(&self, collection: &'a Collection) -> &'a BTreeMap<String, Field> {
        if !collection.fields.is_empty() {
            return &collection.fields;
        }
        self.seen
            .and_then(|seen| seen.fields.get(&collection.name))
            .unwrap_or(&NO_FIELDS)
    }

    /// Whether a collection's fields come from results, not a schema.
    fn guessed(&self, collection: &str) -> bool {
        self.all
            .get(collection)
            .is_some_and(|collection| collection.fields.is_empty())
    }

    fn sample(&self, collection: &str, path: &str) -> Option<&'a str> {
        self.seen?
            .samples
            .get(&(collection.to_owned(), path.to_owned()))
            .map(String::as_str)
    }

    /// Children of the field reached by `segments`, merged across the scope.
    fn children(&self, segments: &[String]) -> BTreeMap<String, (&'a Field, String)> {
        let mut out = BTreeMap::new();
        for collection in &self.scope {
            if let Some(fields) = self.walk(self.fields_of(collection), segments) {
                for (name, field) in fields {
                    out.entry(name.clone())
                        .or_insert_with(|| (field, collection.name.clone()));
                }
            }
        }
        out
    }

    fn walk(
        &self,
        fields: &'a BTreeMap<String, Field>,
        segments: &[String],
    ) -> Option<&'a BTreeMap<String, Field>> {
        let Some((first, rest)) = segments.split_first() else {
            return Some(fields);
        };
        let field = fields.get(first)?;
        self.walk(self.properties(field)?, rest)
    }

    /// The members a path can continue into: object properties, the items
    /// of an array, or the fields of a related collection.
    fn properties(&self, field: &'a Field) -> Option<&'a BTreeMap<String, Field>> {
        let field = items(field);
        if let Some(target) = &field.relation {
            return self
                .all
                .get(target)
                .map(|collection| self.fields_of(collection));
        }
        (!field.properties.is_empty()).then_some(&field.properties)
    }

    /// The field at a complete path, used to suggest its values.
    fn field(&self, segments: &[String]) -> Option<&'a Field> {
        let (last, parents) = segments.split_last()?;
        self.scope.iter().find_map(|collection| {
            self.walk(self.fields_of(collection), parents)
                .and_then(|fields| fields.get(last))
        })
    }
}


/// Array fields are traversed implicitly; describe their items instead.
fn items(field: &Field) -> &Field {
    match (&field.kind, field.items.as_deref()) {
        (ValueType::Array, Some(items)) => items,
        _ => field,
    }
}

fn type_name(field: &Field) -> String {
    let item = items(field);
    if let Some(target) = &item.relation {
        return format!("→ {target}");
    }
    let base = match item.kind {
        ValueType::Object => "object",
        ValueType::Array => "array",
        ValueType::String if item.enum_values.is_some() => "enum",
        ValueType::String => "text",
        ValueType::Number => "number",
        ValueType::Integer => "integer",
        ValueType::Boolean => "boolean",
        ValueType::Null => "null",
        ValueType::Any => "any",
    };
    if field.kind == ValueType::Array {
        format!("{base}[]")
    } else {
        base.into()
    }
}

fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_alphanumeric())
}

/// Characters that can belong to a field path token.
fn path_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '.' | '@' | '[' | ']' | '*' | ':')
}

/// Split a typed path into complete segments and the partial last one.
/// Bracketed keys and indices count as segments; returns `None` when the
/// path cannot be understood.
fn split_path(path: &str) -> Option<(Vec<String>, String)> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '.' => segments.push(std::mem::take(&mut current)),
            '[' => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
                let mut inside = String::new();
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    inside.push(next);
                }
                let inside = inside.trim();
                // `[*]` and `[0]` stay on the same field.
                if let Some(key) = inside
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .or_else(|| {
                        inside
                            .strip_prefix('\'')
                            .and_then(|rest| rest.strip_suffix('\''))
                    })
                {
                    segments.push(key.into());
                }
                if chars.peek() == Some(&'.') {
                    chars.next();
                }
            }
            _ => current.push(ch),
        }
    }
    Some((segments, current))
}

fn join_path(segments: &[String], name: &str) -> String {
    let mut out = String::new();
    for segment in segments.iter().map(String::as_str).chain([name]) {
        if identifier(segment) {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(segment);
        } else {
            out.push_str(&format!("[{}]", serde_json::to_string(segment).unwrap()));
        }
    }
    out
}


/// Rank by prefix, then substring, both case-insensitive.
fn filter(items: Vec<Candidate>, typed: &str) -> Vec<Candidate> {
    let typed = typed.to_lowercase();
    let mut ranked = items
        .into_iter()
        .filter_map(|item| {
            let label = item.label.to_lowercase();
            let rank = if label.starts_with(&typed) {
                0
            } else if label.contains(&typed) {
                1
            } else if subsequence(&label, &typed) {
                // Fuzzy: `prtit` finds `preferred_title`.
                2
            } else {
                return None;
            };
            Some((rank, item))
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, item)| item).take(60).collect()
}


/// Whether the characters of `typed` appear in `label` in order.
fn subsequence(label: &str, typed: &str) -> bool {
    let mut label = label.chars();
    typed.chars().all(|wanted| label.any(|ch| ch == wanted))
}


/// What the text before the token says about the token.
enum Context {
    /// A field, system field, function, or keyword can start here.
    Term,
    /// A value after `path op`.
    Value(String),
    /// After a complete path: an operator follows.
    Operator(String),
    /// After a value: a connective or `SORT BY` follows.
    Connective,
    /// A sort field.
    SortField,
    /// After a sort field: a direction or `&&`.
    SortDirection,
}

/// Inside quotes, completion offers string values of the compared field.
struct Quoted {
    start: usize,
    path: String,
}

fn quoted(chars: &[char], cursor: usize) -> Option<Quoted> {
    let mut quote: Option<(char, usize)> = None;
    let mut escaped = false;
    for (index, &ch) in chars[..cursor].iter().enumerate() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if let Some((open, _)) = quote {
            if ch == open {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some((ch, index));
        }
    }
    let (_, start) = quote?;
    let before = chars[..start].iter().collect::<String>();
    let path = compared_path(before.trim_end())?;
    Some(Quoted { start, path })
}

/// For text ending in `path op`, the path.
fn compared_path(before: &str) -> Option<String> {
    let op_start = before
        .char_indices()
        .rev()
        .take_while(|(_, ch)| matches!(ch, '=' | '!' | '<' | '>' | '~' | '?'))
        .last()
        .map(|(index, _)| index)?;
    let path_text = before[..op_start].trim_end();
    let path_start = path_text
        .char_indices()
        .rev()
        .take_while(|(_, ch)| path_char(*ch) || *ch == '"' || *ch == '\'')
        .last()
        .map_or(path_text.len(), |(index, _)| index);
    let path = &path_text[path_start..];
    (!path.is_empty()).then(|| path.split(':').next().unwrap_or(path).to_string())
}

fn sort_clause_start(before: &str) -> Option<usize> {
    let upper = before.to_uppercase();
    upper.rfind("SORT BY").map(|index| index + "SORT BY".len())
}

fn context(before: &str) -> Context {
    let trimmed = before.trim_end();
    if let Some(start) = sort_clause_start(before) {
        let clause = before[start..].trim_end();
        let last = clause.rsplit("&&").next().unwrap_or("").trim();
        return if last.is_empty() || !before.ends_with(char::is_whitespace) {
            Context::SortField
        } else {
            Context::SortDirection
        };
    }
    if trimmed.is_empty()
        || trimmed.ends_with("&&")
        || trimmed.ends_with("||")
        || trimmed.ends_with('(')
        || trimmed.ends_with('!') && !trimmed.ends_with("!=")
    {
        return Context::Term;
    }
    if let Some(path) = compared_path(trimmed)
        && trimmed.ends_with(['=', '<', '>', '~'])
    {
        return Context::Value(path);
    }
    // Text ends with a token followed by whitespace.
    let last = trimmed
        .rsplit(|ch: char| ch.is_whitespace())
        .next()
        .unwrap_or("");
    let value = last.ends_with(['\'', '"', ')'])
        || last.parse::<f64>().is_ok()
        || matches!(last, "true" | "false" | "null")
        || last.starts_with("@")
            && !last.chars().skip(1).any(|ch| ch == '.')
            && MACROS.iter().any(|(name, _)| *name == last)
        || last.len() >= 7 && last.as_bytes().get(4) == Some(&b'-');
    if value {
        // `@size` and other system fields are paths, not values.
        if SYSTEM_FIELDS.iter().any(|(name, _)| *name == last) {
            return Context::Operator(last.to_string());
        }
        let before_last = trimmed[..trimmed.len() - last.len()].trim_end();
        if before_last.ends_with(['=', '<', '>', '~']) {
            return Context::Connective;
        }
    }
    if last.chars().all(path_char) {
        return Context::Operator(last.to_string());
    }
    Context::Connective
}


/// Whether suggestions should appear without Tab while typing: always,
/// except on an empty line. The list then narrows with every key.
pub(super) fn worth_showing(text: &str) -> bool {
    !text.trim().is_empty()
}


/// Completions for the filter line at `cursor` (a character index).
pub(super) fn complete(text: &str, cursor: usize, schema: &Schema<'_>) -> Option<Completion> {
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    if let Some(Quoted { start, path }) = quoted(&chars, cursor) {
        let typed = chars[start + 1..cursor].iter().collect::<String>();
        let quote = chars[start];
        let end = cursor + usize::from(chars.get(cursor) == Some(&quote));
        let items = values(schema, &path)
            .into_iter()
            .filter(|item| item.insert.starts_with(['\'', '"']))
            .map(|mut item| {
                item.insert = format!("{quote}{}{quote}", item.label);
                item
            })
            .collect();
        let items = filter(items, &typed);
        return (!items.is_empty()).then_some(Completion { start, end, items });
    }
    let start = (0..cursor)
        .rev()
        .take_while(|&index| path_char(chars[index]) || chars[index] == '"')
        .last()
        .unwrap_or(cursor);
    let mut end = cursor;
    while end < chars.len() && path_char(chars[end]) {
        end += 1;
    }
    let token = chars[start..cursor].iter().collect::<String>();
    let before = chars[..start].iter().collect::<String>();
    let mut items = match context(&before) {
        Context::Term => {
            if let Some((path, modifier)) = token.rsplit_once(':') {
                let base = start + path.chars().count() + 1;
                let items = MODIFIERS
                    .iter()
                    .map(|(name, detail)| Candidate {
                        insert: (*name).into(),
                        label: (*name).into(),
                        detail: (*detail).into(),
                        kind: Kind::Keyword,
                    })
                    .collect();
                let items = filter(items, modifier);
                return (!items.is_empty()).then_some(Completion {
                    start: base,
                    end,
                    items,
                });
            }
            let mut items = fields(schema, &token);
            if !token.contains(['.', '[']) {
                items.extend(system_fields());
                items.extend(["exists(", "missing("].map(|name| Candidate {
                    insert: name.into(),
                    label: name.into(),
                    detail: "presence test".into(),
                    kind: Kind::Function,
                }));
            }
            items
        }
        Context::SortField => {
            let mut items = fields(schema, &token);
            if !token.contains(['.', '[']) {
                items.extend(
                    system_fields()
                        .into_iter()
                        .filter(|item| item.label != "@status"),
                );
            }
            items
        }
        Context::SortDirection => keywords(&["ASC", "DESC", "&&"]),
        Context::Value(path) => {
            let mut items = values(schema, &path);
            // Date macros compare with date strings and timestamps only.
            let dated = match path.as_str() {
                "@modified" | "@created" => true,
                path if path.starts_with('@') => false,
                path => lookup(schema, path).is_none_or(|field| {
                    let field = items_of(field);
                    matches!(field.kind, ValueType::String | ValueType::Any)
                        && field.enum_values.is_none()
                }),
            };
            // Real values from the index show whether the field holds dates.
            let counted = items
                .iter()
                .filter(|item| item.detail.ends_with(" records"))
                .collect::<Vec<_>>();
            let dated = dated
                && (counted.is_empty()
                    || counted.iter().any(|item| {
                        let label = item.label.as_bytes();
                        label.len() >= 7
                            && label[..4].iter().all(u8::is_ascii_digit)
                            && label[4] == b'-'
                    }));
            items.extend(
                MACROS
                    .iter()
                    .filter(|_| dated)
                    .map(|(name, detail)| Candidate {
                        insert: (*name).into(),
                        label: (*name).into(),
                        detail: (*detail).into(),
                        kind: Kind::Macro,
                    }),
            );
            items
        }
        Context::Operator(path) => operators(schema, &path)
            .into_iter()
            .map(|(op, detail)| Candidate {
                insert: format!("{op} "),
                label: (*op).into(),
                detail: (*detail).into(),
                kind: Kind::Operator,
            })
            .collect(),
        Context::Connective => keywords(&["&&", "||", "SORT BY"]),
    };
    // Operators and connectives start a new token after whitespace.
    let typed = match items.first().map(|item| item.kind) {
        Some(Kind::Operator | Kind::Keyword) if token.is_empty() => String::new(),
        _ => token
            .split(['.', '['])
            .next_back()
            .unwrap_or("")
            .to_string(),
    };
    items = filter(items, &typed);
    // The only suggestion is what is already typed: step into its fields
    // if it has any, otherwise there is nothing to add.
    if items.len() == 1 && items[0].insert.trim_end() == token {
        if !matches!(items[0].kind, Kind::Field | Kind::Relation) {
            return None;
        }
        let deeper = chars[..cursor]
            .iter()
            .chain(['.'].iter())
            .chain(chars[cursor..].iter())
            .collect::<String>();
        let inner = complete(&deeper, cursor + 1, schema)?;
        return (inner.start == start).then_some(Completion {
            start,
            end,
            items: inner.items,
        });
    }
    (!items.is_empty()).then_some(Completion { start, end, items })
}

fn keywords(names: &[&str]) -> Vec<Candidate> {
    names
        .iter()
        .map(|name| Candidate {
            insert: format!("{name} "),
            label: (*name).into(),
            detail: String::new(),
            kind: Kind::Keyword,
        })
        .collect()
}

fn system_fields() -> Vec<Candidate> {
    SYSTEM_FIELDS
        .iter()
        .map(|(name, detail)| Candidate {
            insert: (*name).into(),
            label: (*name).into(),
            detail: (*detail).into(),
            kind: Kind::System,
        })
        .collect()
}

/// Field candidates for a partial path; the label is the last segment.
fn fields(schema: &Schema<'_>, token: &str) -> Vec<Candidate> {
    let Some((segments, _)) = split_path(token) else {
        return Vec::new();
    };
    schema
        .children(&segments)
        .into_iter()
        .map(|(name, (field, collection))| {
            let item = items(field);
            let kind = if item.relation.is_some() {
                Kind::Relation
            } else {
                Kind::Field
            };
            let mut detail = type_name(field);
            if schema.scope.len() > 1 {
                detail.push_str(&format!(" · {collection}"));
            }
            if schema.guessed(&collection) {
                detail.push_str(" · seen");
            }
            let dotted = segments
                .iter()
                .chain(std::iter::once(&name))
                .cloned()
                .collect::<Vec<_>>()
                .join(".");
            if let Some(sample) = schema.sample(&collection, &dotted) {
                detail.push_str(&format!(" · e.g. {sample}"));
            }
            Candidate {
                insert: join_path(&segments, &name),
                label: name,
                detail,
                kind,
            }
        })
        .collect()
}

/// Values a compared field can take: enum members, booleans, statuses.
/// The schema field a typed path names, if known.
fn lookup<'a>(schema: &Schema<'a>, path: &str) -> Option<&'a Field> {
    let (mut segments, last) = split_path(path)?;
    if !last.is_empty() {
        segments.push(last);
    }
    schema.field(&segments)
}

fn items_of(field: &Field) -> &Field {
    items(field)
}

/// Operators that make sense for the field's type; all of them when the
/// field is unknown.
fn operators(schema: &Schema<'_>, path: &str) -> Vec<(&'static str, &'static str)> {
    let pick = |names: &[&str]| {
        OPERATORS
            .iter()
            .copied()
            .filter(|(op, _)| names.contains(op))
            .collect::<Vec<_>>()
    };
    let ordered = ["=", "!=", ">", ">=", "<", "<="];
    let text = ["=", "!=", "~", "!~", ">", ">=", "<", "<="];
    let field = lookup(schema, path);
    let (field, many) = match path {
        "@size" | "@modified" | "@created" => return pick(&ordered),
        "@id" | "@collection" | "@path" | "@status" => return pick(&text),
        _ => match field {
            Some(field) => (
                items(field),
                field.kind == ValueType::Array || path.contains("[*]"),
            ),
            None => return OPERATORS.to_vec(),
        },
    };
    let mut names = match field.kind {
        ValueType::Number | ValueType::Integer => ordered.to_vec(),
        ValueType::Boolean => vec!["=", "!="],
        ValueType::String if field.enum_values.is_some() => vec!["=", "!=", "~", "!~"],
        ValueType::String => text.to_vec(),
        _ => return OPERATORS.to_vec(),
    };
    if many {
        names.extend(["?=", "?!=", "?~", "?>"]);
    }
    pick(&names)
}

fn values(schema: &Schema<'_>, path: &str) -> Vec<Candidate> {
    let value = |insert: String, label: &str, detail: &str| Candidate {
        insert,
        label: label.into(),
        detail: detail.into(),
        kind: Kind::Value,
    };
    if path == "@status" {
        return ["valid", "invalid", "no_schema", "incorrect_json"]
            .into_iter()
            .map(|status| value(format!("'{status}'"), status, "status"))
            .collect();
    }
    let mut out = Vec::new();
    // The values that actually occur, most common first, from the index.
    if !path.starts_with('@') {
        for (found, records) in schema.top_values(path) {
            let (insert, label) = match &found {
                Value::String(text) => (quote(text), text.clone()),
                other => (other.to_string(), other.to_string()),
            };
            out.push(value(insert, &label, &format!("{records} records")));
        }
    }
    let Some((mut segments, last)) = split_path(path) else {
        return out;
    };
    if !last.is_empty() {
        segments.push(last);
    }
    let Some(field) = schema.field(&segments) else {
        return out;
    };
    let field = items(field);
    let mut push = |candidate: Candidate| {
        if !out
            .iter()
            .any(|known: &Candidate| known.insert == candidate.insert)
        {
            out.push(candidate);
        }
    };
    if let Some(members) = &field.enum_values {
        for member in members {
            match member {
                Value::String(text) => push(value(quote(text), text, "enum")),
                other if !other.is_null() => {
                    push(value(other.to_string(), &other.to_string(), "enum"));
                }
                _ => {}
            }
        }
    }
    if field.enum_values.is_none()
        && let Some(suggested) = &field.suggested_values
    {
        for text in suggested {
            push(value(quote(text), text, "seen"));
        }
    }
    if field.kind == ValueType::Boolean {
        push(value("true".into(), "true", "boolean"));
        push(value("false".into(), "false", "boolean"));
    }
    if field.nullable {
        push(value("null".into(), "null", "null"));
    }
    out
}


/// A single-quoted filter string literal.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}


#[cfg(test)]
#[path = "complete.test.rs"]
mod tests;
