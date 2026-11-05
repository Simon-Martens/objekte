//! Compile full-text patterns and identify matching document paths.

use super::*;
use crate::{
    TextMode,
    extensions::{BindingContext, BoundTextQuery, ScalarValue},
    text::{TextPattern, visit_json_text},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_BINDING_ID: AtomicU64 = AtomicU64::new(1);

type FormatBindings = [Option<bool>; 4];

fn format_index(format: crate::TextFormat) -> usize {
    match format {
        crate::TextFormat::Plain => 0,
        crate::TextFormat::Markdown => 1,
        crate::TextFormat::Html => 2,
        crate::TextFormat::Xml => 3,
    }
}

fn format_name(format: crate::TextFormat) -> &'static str {
    match format {
        crate::TextFormat::Plain => "plain",
        crate::TextFormat::Markdown => "markdown",
        crate::TextFormat::Html => "html",
        crate::TextFormat::Xml => "xml",
    }
}


/// Immutable applicability compiled once for the collection and schema.
pub(super) struct TextBindingPlan<'a> {
    id: u64,
    collection: &'a str,
    schema: Option<&'a Value>,
    known: HashMap<String, FormatBindings>,
}


impl<'a> TextBindingPlan<'a> {
    pub(super) fn new(
        collection: &'a str,
        schema: Option<&'a Value>,
        rules: &TextRules,
        extension: &BoundTextQuery,
    ) -> Self {
        let mut known = HashMap::<String, FormatBindings>::new();
        for (path, format) in rules.declared_text_paths() {
            let allowed = extension.representation.binds(&BindingContext {
                collection,
                path: &path,
                format: format_name(format),
                schema,
            });
            known.entry(path).or_insert([None; 4])[format_index(format)] = Some(allowed);
        }
        Self {
            id: NEXT_BINDING_ID.fetch_add(1, Ordering::Relaxed),
            collection,
            schema,
            known,
        }
    }

    fn allows(
        &self,
        path: &str,
        format: crate::TextFormat,
        extension: &BoundTextQuery,
        scratch: &mut ExtensionScratch,
    ) -> bool {
        let index = format_index(format);
        if let Some(allowed) = self.known.get(path).and_then(|values| values[index]) {
            return allowed;
        }
        if scratch.binding_id != self.id {
            scratch.binding_id = self.id;
            scratch.unanticipated.clear();
        }
        if let Some(allowed) = scratch
            .unanticipated
            .get(path)
            .and_then(|values| values[index])
        {
            return allowed;
        }
        let allowed = extension.representation.binds(&BindingContext {
            collection: self.collection,
            path,
            format: format_name(format),
            schema: self.schema,
        });
        // Arbitrary schemaless keys cannot retain unbounded worker memory.
        if scratch.unanticipated.len() >= 4096 {
            scratch.unanticipated.clear();
        }
        scratch
            .unanticipated
            .entry(path.to_owned())
            .or_insert([None; 4])[index] = Some(allowed);
        allowed
    }
}


#[derive(Default)]
struct ExtensionScratch {
    rendered: String,
    scanner: String,
    binding_id: u64,
    unanticipated: HashMap<String, FormatBindings>,
}


thread_local! {
    static EXTENSION_SCRATCH: RefCell<ExtensionScratch> = RefCell::new(ExtensionScratch::default());
}

#[cfg(test)]
#[path = "matcher.test.rs"]
mod tests;


/// A request's compiled pattern and conservative bigram prefilter. Whether
/// text matches is decided by the shared [`TextPattern`].
pub(super) struct TextMatcher {
    pattern: TextPattern,
    pub(super) candidate_pattern: Option<String>,
    pub(super) candidate_literal: bool,
    pub(super) raw: bool,
}


/// Canonical query fields and, for returned hits, concrete indexed paths.
pub(super) struct TextMatches {
    pub(super) fields: Vec<String>,
    pub(super) matched_paths: Vec<String>,
}


fn indexed_path(path: &str, indices: &[(usize, usize)]) -> String {
    if path.is_empty() {
        return "$value".into();
    }
    let mut result = String::with_capacity(path.len() + indices.len() * 3);
    if indices.first().is_some_and(|(offset, _)| *offset == 0) {
        result.push_str("$value");
    }
    let mut start = 0;
    for &(offset, index) in indices {
        // Offsets locate traversal-inserted [] markers. Splitting on the text
        // "[]" would mistake the same characters inside a JSON field name.
        result.push_str(&path[start..offset]);
        result.push('[');
        result.push_str(&index.to_string());
        result.push(']');
        start = offset + 2;
    }
    result.push_str(&path[start..]);
    result
}


impl TextMatcher {
    pub(super) fn matching_fields_extension(
        document: &Value,
        rules: &TextRules,
        selected: Option<&BTreeSet<&str>>,
        include_indices: bool,
        extension: &BoundTextQuery,
        binding: &TextBindingPlan<'_>,
    ) -> Result<TextMatches> {
        EXTENSION_SCRATCH.with(|cell| match cell.try_borrow_mut() {
            Ok(mut scratch) => Self::matching_fields_extension_with_scratch(
                document,
                rules,
                selected,
                include_indices,
                extension,
                binding,
                &mut scratch,
            ),
            Err(_) => Self::matching_fields_extension_with_scratch(
                document,
                rules,
                selected,
                include_indices,
                extension,
                binding,
                &mut ExtensionScratch::default(),
            ),
        })
    }

    fn matching_fields_extension_with_scratch(
        document: &Value,
        rules: &TextRules,
        selected: Option<&BTreeSet<&str>>,
        include_indices: bool,
        extension: &BoundTextQuery,
        binding: &TextBindingPlan<'_>,
        scratch: &mut ExtensionScratch,
    ) -> Result<TextMatches> {
        let mut fields = BTreeSet::new();
        let mut matched_paths = Vec::new();
        let mut failure = None;
        visit_json_text(
            document,
            rules,
            include_indices,
            &mut |name, indices, value, format| {
                if failure.is_some() || selected.is_some_and(|selected| !selected.contains(name)) {
                    return;
                }
                if !binding.allows(name, format, extension, scratch) {
                    return;
                }
                let matched = (|| {
                    let rendered = &mut scratch.rendered;
                    let scanner = &mut scratch.scanner;
                    rendered.clear();
                    scanner.clear();
                    match extension.representation.render(value, rendered)? {
                        Some(text)
                            if extension.preparation
                                == crate::extensions::TextPreparation::VisibleNormalized =>
                        {
                            extension
                                .operator
                                .matches_prepared(ScalarValue::Text(text), scanner)
                        }
                        Some(text) => extension.operator.matches(ScalarValue::Text(text), scanner),
                        None => Ok(false),
                    }
                })();
                match matched {
                    Ok(true) => {
                        fields.insert(name.to_owned());
                        if include_indices {
                            matched_paths.push(indexed_path(name, indices));
                        }
                    }
                    Ok(false) => {}
                    Err(error) => failure = Some(error),
                }
            },
        );
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(TextMatches {
            fields: fields.into_iter().collect(),
            matched_paths,
        })
    }

    pub(super) fn matches_value(&self, value: &str, format: crate::TextFormat) -> bool {
        self.pattern.matches(value, format)
    }

    pub(super) fn compile(request: &TextQuery) -> Result<Self> {
        let pattern = TextPattern::compile(&request.pattern, request.mode, request.case_sensitive)?;
        let raw = pattern.mode() != TextMode::Word;
        let word_mode = pattern.mode() != TextMode::Regex;
        // Word modes narrow by the needle as the values are indexed; a regex
        // is planned as given, with its own case flags.
        let candidate_base = pattern.needle().unwrap_or(pattern.source());
        let candidate_pattern = Some(candidate_base.to_owned());
        Ok(Self {
            candidate_pattern,
            candidate_literal: word_mode,
            raw,
            pattern,
        })
    }

    pub(super) fn matching_fields(
        &self,
        document: &Value,
        rules: &TextRules,
        selected: Option<&BTreeSet<&str>>,
        include_indices: bool,
    ) -> TextMatches {
        let mut fields = BTreeSet::new();
        let mut matched_paths = Vec::new();
        visit_json_text(
            document,
            rules,
            include_indices,
            &mut |name, indices, value, format| {
                if selected.is_some_and(|selected| !selected.contains(name)) {
                    return;
                }
                if self.matches_value(value, format) {
                    fields.insert(name.to_owned());
                    if include_indices {
                        matched_paths.push(indexed_path(name, indices));
                    }
                }
            },
        );
        TextMatches {
            fields: fields.into_iter().collect(),
            matched_paths,
        }
    }

    pub(super) fn matches_malformed(&self, bytes: &[u8]) -> bool {
        let text = crate::text::malformed_search_text(bytes);
        self.matches_malformed_text(&text)
    }

    pub(super) fn matches_malformed_text(&self, text: &str) -> bool {
        self.pattern.matches(text, crate::TextFormat::Plain)
    }

    /// Normal postings retain scalar values but do not normalize whitespace.
    /// Broad or whitespace-sensitive patterns therefore scan all file slots.
    pub(super) fn regular_candidate_pattern(&self) -> Option<&str> {
        let pattern = self.candidate_pattern.as_deref()?;
        // A normalized word pattern has no punctuation and single spaces.
        // Each of its words appears unchanged in the source text, so the
        // raw index can narrow by the longest one; verification checks the
        // whole phrase.
        if self.candidate_literal && !self.raw {
            return pattern
                .split(' ')
                .max_by_key(|word| word.len())
                .filter(|word| !word.is_empty());
        }
        if pattern.chars().any(char::is_whitespace)
            || [r"\s", r"\n", r"\r", r"\t", r"\x", r"\u"]
                .iter()
                .any(|escape| pattern.contains(escape))
        {
            None
        } else {
            Some(pattern)
        }
    }
}
