//! Locale-aware sort keys and deterministic scalar ordering.
//!
//! Parsing and bounded result retention live in small neighboring modules.

mod buffer;
mod parser;

pub(crate) use buffer::SortBuffer;
pub use parser::{parse_sort_terms, split_sort_clause};

use crate::{
    Collection, Direction, Error, JsonFilter, JsonPath, JsonPathStep, Result, Sort,
    json_filter::{decimal_cmp, sonic_values, values},
    schema::SortReduce,
};
use icu_collator::{
    CollatorBorrowed,
    options::{CollatorOptions, Strength},
};
use icu_locale_core::Locale;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{cmp::Ordering, collections::BTreeMap};


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


/// A sort field with its resolved path, reduction rule, and ICU collator.
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

    pub(crate) fn compile_fields_with_language(
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
            // Secondary strength keeps case variants together while retaining
            // accent distinctions such as German A and Ä.
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

    pub(crate) fn term_count(&self) -> usize {
        self.terms.len()
    }

    pub(crate) fn project(&self, id: &str, data: &Value) -> Result<Vec<SortValue>> {
        self.terms
            .iter()
            .map(|term| {
                if term.path.is_empty() {
                    return Ok(SortValue::String(id.as_bytes().to_vec()));
                }
                term.select(
                    values(data, &term.path, true),
                    |value| Ok(term.value(value)),
                )
            })
            .collect()
    }

    /// Project directly from the tree used by the other index projections.
    pub(crate) fn project_sonic(&self, id: &str, data: &SonicValue) -> Result<Vec<SortValue>> {
        let mut output = self.missing_keys();
        self.project_sonic_into(id, data, &mut output)?;
        Ok(output)
    }

    pub(crate) fn project_sonic_into(
        &self,
        id: &str,
        data: &SonicValue,
        output: &mut [SortValue],
    ) -> Result<()> {
        debug_assert_eq!(output.len(), self.terms.len());
        for (term, target) in self.terms.iter().zip(output) {
            *target = if term.path.is_empty() {
                SortValue::String(id.as_bytes().to_vec())
            } else {
                term.select(sonic_values(data, &term.path, true), |value| {
                    term.value_sonic(value)
                })?
            };
        }
        Ok(())
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
    fn select<V>(
        &self,
        values: Vec<&V>,
        convert: impl Fn(&V) -> Result<SortValue>,
    ) -> Result<SortValue> {
        match values.len() {
            0 => return Ok(SortValue::Missing),
            1 => return convert(values[0]),
            _ => {}
        }
        let Some(reduce) = self.reduce else {
            return Err(Error::Request(
                "sorting a multivalued path requires x-sort-reduce in the schema".into(),
            ));
        };
        if reduce == SortReduce::First {
            return convert(values[0]);
        }
        if reduce == SortReduce::Last {
            return convert(values[values.len() - 1]);
        }
        let mut values = values.into_iter();
        let mut best = convert(values.next().expect("nonempty sort values"))?;
        for value in values {
            let next = convert(value)?;
            let order = next.compare(&best, Direction::Asc);
            // Iterator::max_by selects the last equal item; min_by keeps
            // the first. Numeric lexemes may compare equal but serialize
            // differently, so this tie rule preserves stored keys.
            if (reduce == SortReduce::Min && order.is_lt())
                || (reduce == SortReduce::Max && !order.is_lt())
            {
                best = next;
            }
        }
        Ok(best)
    }

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

    fn value_sonic(&self, value: &SonicValue) -> Result<SortValue> {
        if let Some(raw) = value.as_raw_number() {
            let number: serde_json::Number = raw
                .as_str()
                .parse()
                .map_err(|error| Error::Cache(format!("cannot project sort number: {error}")))?;
            return Ok(SortValue::Number(number.to_string()));
        }
        Ok(match value.as_ref() {
            ValueRef::Null => SortValue::Null,
            ValueRef::Bool(value) => SortValue::Bool(value),
            ValueRef::String(value) => {
                let mut key = Vec::new();
                self.collator
                    .write_sort_key_to(value, &mut key)
                    .expect("Vec accepts bytes");
                SortValue::String(key)
            }
            ValueRef::Number(_) => unreachable!("raw numbers enabled"),
            _ => SortValue::Invalid,
        })
    }
}
