//! Build index text and scalar keys from one Sonic tree without copying the full JSON tree.

use super::{
    SortIndex, TypedKey, ViewIndex,
    key_summary::KeySummary,
    text::{TextContent, project_text},
};
use crate::{
    Error, Result,
    error::io,
    query::Scalar,
    sorting::SortValue,
    source::{Source, Stamp},
    text::{TextRules, malformed_search_text},
};
use serde_json::Number;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{collections::BTreeMap, io::Read};
use unicode_casefold::UnicodeCaseFold;


/// Decoded text and top-level keys extracted from one source document.
pub(super) struct Projection {
    pub(super) bytes: Vec<u8>,
    pub(super) text: Option<TextContent>,
    pub(super) array_fields: Vec<String>,
    pub(super) present_fields: Vec<usize>,
    pub(super) typed_values: Vec<(usize, TypedKey)>,
    pub(super) view_keys: Vec<Scalar>,
    pub(super) sort_keys: Vec<Vec<crate::sorting::SortValue>>,
}


/// Parsed projection or searchable fallback text for a malformed source file.
pub(super) enum ProjectionOutcome {
    Valid(Projection),
    Malformed(String),
}


/// Optional projections produced during a source-file index scan.
pub(super) struct ProjectionOptions<'a> {
    pub(super) folded_index: bool,
    pub(super) text_rules: Option<&'a TextRules>,
    pub(super) key_summary: Option<&'a mut KeySummary>,
}


#[expect(
    clippy::too_many_arguments,
    reason = "projection borrows each index independently"
)]
pub(super) fn projected_values(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    sort: &SortIndex,
    folded_index: bool,
) -> Result<Option<Projection>> {
    Ok(
        match projected_values_with_text(
            source,
            id,
            stamp,
            indexed_fields,
            presence_fields,
            view,
            sort,
            ProjectionOptions {
                folded_index,
                text_rules: None,
                key_summary: None,
            },
        )? {
            ProjectionOutcome::Valid(projection) => Some(projection),
            ProjectionOutcome::Malformed(_) => None,
        },
    )
}

/// Reuse the same source read and Sonic tree when full-text postings are active.
#[expect(
    clippy::too_many_arguments,
    reason = "projection borrows each index independently"
)]
pub(super) fn projected_values_with_text(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    sort: &SortIndex,
    options: ProjectionOptions<'_>,
) -> Result<ProjectionOutcome> {
    let mut raw = Vec::new();
    projected_values_with_text_buffer(
        source,
        id,
        stamp,
        indexed_fields,
        presence_fields,
        view,
        sort,
        options,
        &mut raw,
    )
}

/// Parallel rebuild workers reuse this buffer only after each owned projection
/// is complete. No returned projection borrows from it.
#[expect(
    clippy::too_many_arguments,
    reason = "projection borrows each index independently"
)]
pub(super) fn projected_values_with_text_buffer(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    sort: &SortIndex,
    options: ProjectionOptions<'_>,
    raw: &mut Vec<u8>,
) -> Result<ProjectionOutcome> {
    projected_values_with_text_buffer_and_sort(
        source,
        id,
        stamp,
        indexed_fields,
        presence_fields,
        view,
        sort,
        options,
        raw,
        None,
        None,
        None,
    )
}

/// Rebuild can write fixed-width sort keys into its worker-owned chunk.
#[expect(
    clippy::too_many_arguments,
    reason = "projection borrows independent indexes and output"
)]
pub(super) fn projected_values_with_text_buffer_and_sort(
    source: &Source,
    id: &str,
    stamp: &Stamp,
    indexed_fields: &[String],
    presence_fields: &[String],
    view: &ViewIndex,
    sort: &SortIndex,
    mut options: ProjectionOptions<'_>,
    raw: &mut Vec<u8>,
    sort_output: Option<&mut [SortValue]>,
    view_output: Option<(&mut [&mut [Scalar]], usize)>,
    postings: Option<(&[crate::JsonPath], &mut [super::posting::ChunkRows], usize)>,
) -> Result<ProjectionOutcome> {
    let path = source.path.join(format!("{id}.json"));
    let mut file = source.file(id)?;
    let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if &before != stamp {
        return Err(Error::Unstable(path));
    }
    raw.clear();
    file.read_to_end(raw).map_err(|e| io(&path, e))?;
    let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
    if after != before || raw.len() as u64 != after.size {
        return Err(Error::Unstable(path));
    }

    let Ok(parsed) = sonic_rs::from_slice::<SonicValue>(raw) else {
        return Ok(ProjectionOutcome::Malformed(malformed_search_text(raw)));
    };
    let mut values = String::new();
    if append_values_except_indexed(&parsed, indexed_fields, &mut values).is_err() {
        return Ok(ProjectionOutcome::Malformed(malformed_search_text(raw)));
    }
    if let Some((paths, rows, slot)) = postings {
        super::posting::project_document(&parsed, slot, paths, rows);
    }
    if options.folded_index && !values.is_ascii() {
        let folded = values.case_fold().collect::<String>();
        if !folded
            .bytes()
            .eq(values.bytes().map(|byte| byte.to_ascii_lowercase()))
        {
            values.push('\u{1f}');
            values.push_str(&folded);
        }
    }

    let fields = top_level_fields(&parsed);
    if let Some(summary) = options.key_summary.as_mut() {
        for name in fields.keys() {
            summary.add(name);
        }
    }
    let array_fields = fields
        .iter()
        .filter(|(_, value)| value.is_array())
        .map(|(name, _)| (*name).to_owned())
        .collect();
    let present_fields = fields
        .keys()
        .filter_map(|name| {
            presence_fields
                .binary_search_by(|field| field.as_str().cmp(name))
                .ok()
        })
        .collect();

    let typed_values = indexed_fields
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            // Absence is a distinct posting from an explicit JSON null.
            // Containers remain unposted because they are not scalar values.
            match fields.get(name.as_str()) {
                Some(value) => TypedKey::from_sonic(value),
                None => Some(TypedKey::Missing),
            }
            .map(|key| (index, key))
        })
        .collect();
    let view_keys = if let Some((columns, within)) = view_output {
        view.project_sonic_into_columns(id, &fields, columns, within);
        Vec::new()
    } else {
        view.project_sonic(id, &fields)
    };
    let sort_keys = if let Some(output) = sort_output {
        sort.project_sonic_into(id, &parsed, output)?;
        Vec::new()
    } else if sort.is_empty() {
        Vec::new()
    } else {
        sort.project_sonic(id, &parsed)?
    };
    Ok(ProjectionOutcome::Valid(Projection {
        bytes: values.into_bytes(),
        text: options
            .text_rules
            .map(|rules| project_text(&parsed, rules, indexed_fields)),
        array_fields,
        present_fields,
        typed_values,
        view_keys,
        sort_keys,
    }))
}

fn top_level_fields(value: &SonicValue) -> BTreeMap<&str, &SonicValue> {
    let mut result = BTreeMap::new();
    if let ValueRef::Object(fields) = value.as_ref() {
        for (name, value) in fields {
            result.insert(name, value);
        }
    }
    result
}

fn exact_number(value: &SonicValue) -> std::result::Result<Number, ()> {
    value
        .as_raw_number()
        .ok_or(())?
        .as_str()
        .parse()
        .map_err(|_| ())
}

fn validate_value(value: &SonicValue, depth: usize) -> std::result::Result<(), ()> {
    if value.as_raw_number().is_some() {
        exact_number(value)?;
        return Ok(());
    }
    match value.as_ref() {
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err(());
            }
            for value in values {
                validate_value(value, depth + 1)?;
            }
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err(());
            }
            for (_, value) in values {
                validate_value(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn valid_document(value: &SonicValue) -> bool {
    validate_value(value, 0).is_ok()
}

fn append_values(
    value: &SonicValue,
    out: &mut String,
    depth: usize,
) -> std::result::Result<(), ()> {
    if value.as_raw_number().is_some() {
        out.push_str(&exact_number(value)?.to_string());
        out.push('\u{1f}');
        return Ok(());
    }
    match value.as_ref() {
        ValueRef::Null => out.push_str("null\u{1f}"),
        ValueRef::Bool(value) => out.push_str(if value { "true\u{1f}" } else { "false\u{1f}" }),
        ValueRef::String(value) => {
            for ch in value.chars() {
                out.push(if ch == '\n' || ch == '\r' {
                    '\u{1f}'
                } else {
                    ch
                });
            }
            out.push('\u{1f}');
        }
        ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        ValueRef::Array(values) => {
            if depth >= 127 {
                return Err(());
            }
            for value in values {
                append_values(value, out, depth + 1)?;
            }
        }
        ValueRef::Object(values) => {
            if depth >= 127 {
                return Err(());
            }
            let mut last = BTreeMap::new();
            for (name, value) in values {
                // The public JSON representation keeps the last duplicate.
                // Validate discarded subtrees too, even though their text is absent.
                if let Some(previous) = last.insert(name, value) {
                    validate_value(previous, depth + 1)?;
                }
            }
            for value in last.values() {
                append_values(value, out, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Keep invalid container values searchable. Only scalar values that have a
/// complete typed posting can be omitted from the general bigram projection.
fn append_values_except_indexed(
    value: &SonicValue,
    indexed_fields: &[String],
    out: &mut String,
) -> std::result::Result<(), ()> {
    let ValueRef::Object(fields) = value.as_ref() else {
        return append_values(value, out, 0);
    };
    let mut last = BTreeMap::new();
    for (name, child) in fields {
        if let Some(previous) = last.insert(name, child) {
            validate_value(previous, 1)?;
        }
    }
    for (name, child) in last {
        if indexed_fields
            .binary_search_by(|field| field.as_str().cmp(name))
            .is_ok()
            && !matches!(child.as_ref(), ValueRef::Array(_) | ValueRef::Object(_))
        {
            validate_value(child, 1)?;
        } else {
            append_values(child, out, 1)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "projection.test.rs"]
mod tests;
