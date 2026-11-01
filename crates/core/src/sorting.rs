//! Query sort syntax, locale-aware keys, and deterministic scalar ordering.

use crate::{
    Collection, Direction, Error, JsonFilter, JsonPath, JsonPathStep, Result, Sort,
    json_filter::{decimal_cmp, values},
    schema::SortReduce,
};
use icu_collator::{
    CollatorBorrowed,
    options::{CollatorOptions, Strength},
};
use icu_locale_core::Locale;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

/// Split a trailing `SORT BY` clause from a field filter or text pattern.
pub fn split_sort_clause(input: &str) -> Result<(String, Vec<Sort>)> {
    let mut quote = None;
    let mut escaped = false;
    let mut candidate = None;
    for (at, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if input[at..]
            .get(..7)
            .is_some_and(|token| token.eq_ignore_ascii_case("SORT BY"))
            && (at == 0 || input[..at].chars().last().is_some_and(char::is_whitespace))
            && input[at + 7..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            candidate = Some(at);
        }
    }
    let Some(at) = candidate else {
        return Ok((input.to_owned(), Vec::new()));
    };
    let before = input[..at].trim_end();
    let clause = input[at + 7..].trim();
    if clause.is_empty() {
        return Err(Error::Request("SORT BY needs at least one field".into()));
    }
    let mut boundaries = vec![0];
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in clause.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if clause[at..].starts_with("&&") {
            boundaries.push(at + 2);
        }
    }
    let mut seen = BTreeSet::new();
    let mut terms = Vec::new();
    for (index, &start) in boundaries.iter().enumerate() {
        let end = boundaries
            .get(index + 1)
            .map_or(clause.len(), |next| next - 2);
        let raw = &clause[start..end];
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(Error::Request("empty SORT BY term".into()));
        }
        let (field, direction) = match raw.rsplit_once(char::is_whitespace) {
            Some((field, word)) if word.eq_ignore_ascii_case("asc") => {
                (field.trim_end(), Direction::Asc)
            }
            Some((field, word)) if word.eq_ignore_ascii_case("desc") => {
                (field.trim_end(), Direction::Desc)
            }
            _ => (raw, Direction::Asc),
        };
        if field != "@id" {
            JsonFilter::parse_path(field)?;
        }
        if !seen.insert(field) {
            return Err(Error::Request(format!("duplicate SORT BY field {field}")));
        }
        terms.push(Sort {
            field: field.into(),
            direction,
        });
    }
    Ok((before.to_owned(), terms))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum SortValue {
    Bool(bool),
    Number(String),
    String(Vec<u8>),
    Null,
    Missing,
    Invalid,
}

impl SortValue {
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Self::Bool(_) => 0,
            Self::Number(_) => 1,
            Self::String(_) => 2,
            Self::Null => 3,
            Self::Missing => 4,
            Self::Invalid => 5,
        }
    }

    pub(crate) fn compare(&self, other: &Self, direction: Direction) -> Ordering {
        if self.rank() != other.rank() {
            return self.rank().cmp(&other.rank());
        }
        let order = match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => decimal_cmp(a, b),
            (Self::String(a), Self::String(b)) => a.cmp(b),
            _ => Ordering::Equal,
        };
        if direction == Direction::Desc && self.rank() < 3 {
            order.reverse()
        } else {
            order
        }
    }
}

struct Term {
    path: JsonPath,
    direction: Direction,
    reduce: Option<SortReduce>,
    collator: CollatorBorrowed<'static>,
}

/// One collection's sort semantics; collections themselves never interleave.
pub(crate) struct SortPlan {
    terms: Vec<Term>,
}

impl SortPlan {
    pub(crate) fn compile(collection: &Collection, sorts: &[Sort]) -> Result<Self> {
        let language = collection
            .schema
            .as_ref()
            .and_then(|schema| schema.field.language.as_deref());
        Self::compile_fields_with_language(&collection.fields, sorts, language)
    }

    pub(crate) fn compile_fields(
        fields: &BTreeMap<String, crate::Field>,
        sorts: &[Sort],
    ) -> Result<Self> {
        Self::compile_fields_with_language(fields, sorts, None)
    }

    fn compile_fields_with_language(
        fields: &BTreeMap<String, crate::Field>,
        sorts: &[Sort],
        default_language: Option<&str>,
    ) -> Result<Self> {
        let mut terms = Vec::with_capacity(sorts.len());
        for sort in sorts {
            let path = if sort.field == "@id" {
                Vec::new()
            } else {
                JsonFilter::parse_path(&sort.field)?
            };
            let mut current: Option<&crate::Field> = None;
            let mut at_root = true;
            let mut language = default_language.unwrap_or("en");
            let mut reduce = None;
            for step in &path {
                let JsonPathStep::Key(name) = step else {
                    if let Some(field) = current
                        && matches!(step, JsonPathStep::Each | JsonPathStep::Index(_))
                    {
                        current = field.items.as_deref();
                    }
                    continue;
                };
                current = if at_root {
                    fields.get(name)
                } else {
                    current.and_then(|field| {
                        field.properties.get(name).or_else(|| {
                            field
                                .items
                                .as_ref()
                                .and_then(|item| item.properties.get(name))
                        })
                    })
                };
                at_root = false;
                if let Some(field) = current {
                    language = field.language.as_deref().unwrap_or(language);
                    reduce = reduce.or(field.sort_reduce);
                }
            }
            let locale: Locale = language
                .parse()
                .map_err(|_| Error::Request(format!("invalid sort language {language}")))?;
            let mut options = CollatorOptions::default();
            options.strength = Some(Strength::Secondary);
            let collator = CollatorBorrowed::try_new(locale.into(), options).map_err(|error| {
                Error::Request(format!("cannot use sort language {language}: {error}"))
            })?;
            terms.push(Term {
                path,
                direction: sort.direction,
                reduce,
                collator,
            });
        }
        Ok(Self { terms })
    }

    pub(crate) fn missing_keys(&self) -> Vec<SortValue> {
        vec![SortValue::Missing; self.terms.len()]
    }

    pub(crate) fn project(&self, id: &str, data: &Value) -> Result<Vec<SortValue>> {
        self.terms
            .iter()
            .map(|term| {
                if term.path.is_empty() {
                    return Ok(SortValue::String(id.as_bytes().to_vec()));
                }
                let found = values(data, &term.path, true);
                if found.is_empty() {
                    return Ok(SortValue::Missing);
                }
                if found.len() == 1 {
                    return Ok(term.value(found[0]));
                }
                let Some(reduce) = term.reduce else {
                    return Err(Error::Request(
                        "sorting a multivalued path requires x-sort-reduce in the schema".into(),
                    ));
                };
                let selected = match reduce {
                    SortReduce::First => term.value(found[0]),
                    SortReduce::Last => term.value(found[found.len() - 1]),
                    SortReduce::Min => found
                        .into_iter()
                        .map(|v| term.value(v))
                        .min_by(|a, b| a.compare(b, Direction::Asc))
                        .unwrap(),
                    SortReduce::Max => found
                        .into_iter()
                        .map(|v| term.value(v))
                        .max_by(|a, b| a.compare(b, Direction::Asc))
                        .unwrap(),
                };
                Ok(selected)
            })
            .collect()
    }

    pub(crate) fn compare(
        &self,
        a: &[SortValue],
        a_id: &str,
        b: &[SortValue],
        b_id: &str,
    ) -> Ordering {
        self.terms
            .iter()
            .zip(a)
            .zip(b)
            .find_map(|((term, a), b)| {
                let order = a.compare(b, term.direction);
                (order != Ordering::Equal).then_some(order)
            })
            .unwrap_or_else(|| a_id.cmp(b_id))
    }
}

impl Term {
    fn value(&self, value: &Value) -> SortValue {
        match value {
            Value::Null => SortValue::Null,
            Value::Bool(value) => SortValue::Bool(*value),
            Value::Number(value) => SortValue::Number(value.to_string()),
            Value::String(value) => {
                let mut key = Vec::new();
                self.collator
                    .write_sort_key_to(value, &mut key)
                    .expect("Vec accepts bytes");
                SortValue::String(key)
            }
            _ => SortValue::Invalid,
        }
    }
}

struct Entry<T> {
    collection: String,
    id: String,
    keys: Vec<SortValue>,
    value: T,
}

/// Keeps only the best accessible prefix while an unindexed scan proceeds.
pub(crate) struct SortBuffer<T> {
    plans: BTreeMap<String, SortPlan>,
    entries: Vec<Entry<T>>,
    keep: usize,
    offset: usize,
    page_size: usize,
    total_limit: usize,
    seen: usize,
}

impl<T> SortBuffer<T> {
    pub(crate) fn new(
        plans: BTreeMap<String, SortPlan>,
        offset: usize,
        page_size: usize,
        total_limit: usize,
    ) -> Self {
        Self {
            plans,
            entries: Vec::new(),
            keep: offset.saturating_add(page_size).min(total_limit),
            offset,
            page_size,
            total_limit,
            seen: 0,
        }
    }

    pub(crate) fn insert(
        &mut self,
        collection: &str,
        id: &str,
        data: &Value,
        value: impl FnOnce() -> T,
    ) -> Result<bool> {
        self.seen += 1;
        if self.keep == 0 {
            return Ok(false);
        }
        let plan = &self.plans[collection];
        let keys = plan.project(id, data)?;
        let position =
            self.entries
                .partition_point(|entry| match entry.collection.as_str().cmp(collection) {
                    Ordering::Less => true,
                    Ordering::Greater => false,
                    Ordering::Equal => plan.compare(&entry.keys, &entry.id, &keys, id).is_lt(),
                });
        if position >= self.keep {
            return Ok(false);
        }
        self.entries.insert(
            position,
            Entry {
                collection: collection.into(),
                id: id.into(),
                keys,
                value: value(),
            },
        );
        if self.entries.len() > self.keep {
            self.entries.pop();
        }
        Ok(true)
    }

    pub(crate) fn seen(&self) -> usize {
        self.seen
    }
    pub(crate) fn total(&self, count: bool) -> Option<usize> {
        count.then_some(self.seen.min(self.total_limit))
    }
    pub(crate) fn truncated(&self) -> bool {
        self.seen > self.total_limit
    }
    pub(crate) fn page(&self) -> impl Iterator<Item = &T> {
        self.entries
            .iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| &entry.value)
    }
    pub(crate) fn into_page(self) -> Vec<T> {
        self.entries
            .into_iter()
            .skip(self.offset)
            .take(self.page_size)
            .map(|entry| entry.value)
            .collect()
    }
}
