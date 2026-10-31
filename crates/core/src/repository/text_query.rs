//! Full-text search over all included JSON leaf values.

use super::*;
use crate::{
    TextHit, TextMode, TextQuery, TextQueryResult, TextQueryStreamSummary,
    text::{TextRules, render_text, visit_json_text},
};
use regex::Regex;
use unicode_casefold::UnicodeCaseFold;


/// Compiled matcher and conservative bigram prefilter for one request.
struct TextMatcher {
    regex: Regex,
    candidate_pattern: Option<String>,
    candidate_literal: bool,
    raw: bool,
    fold_word: bool,
}


/// Canonical query fields and, for returned hits, concrete indexed paths.
struct TextMatches {
    fields: Vec<String>,
    matched_paths: Vec<String>,
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
    fn compile(request: &TextQuery) -> Result<Self> {
        if request.pattern.len() > 16 * 1024 || request.pattern.is_empty() {
            return Err(Error::Request(
                "text pattern must contain 1–16384 bytes".into(),
            ));
        }
        let raw = matches!(request.mode, TextMode::WordRaw | TextMode::RegexRaw);
        let word_mode = matches!(request.mode, TextMode::Word | TextMode::WordRaw);
        let fold_word = word_mode && !request.case_sensitive;
        let mut candidate_base = None;
        let pattern = if word_mode {
            let word = if raw {
                request.pattern.clone()
            } else {
                render_text(&request.pattern, crate::TextFormat::Plain)
                    .expect("plain rendering always succeeds")
            };
            if word.trim().is_empty() {
                return Err(Error::Request(
                    "word search needs non-whitespace text".into(),
                ));
            }
            let word = if fold_word {
                word.case_fold().collect::<String>()
            } else {
                word
            };
            let literal = regex::escape(&word);
            candidate_base = Some(word);
            format!(r"\b{literal}\b")
        } else {
            request.pattern.clone()
        };
        let regex = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!request.case_sensitive && !fold_word)
            .build()
            .map_err(|error| Error::Request(format!("invalid text regex: {error}")))?;
        let candidate_base = candidate_base.as_deref().unwrap_or(&pattern);
        let candidate_pattern = if word_mode {
            Some(candidate_base.to_owned())
        } else if !candidate_base.is_ascii() {
            // Unicode regex folding may select byte sequences absent from a
            // case-folded projection. Keep those searches conservative.
            None
        } else if !request.case_sensitive {
            Some(format!("(?i:{candidate_base})"))
        } else {
            Some(candidate_base.to_owned())
        };
        Ok(Self {
            regex,
            candidate_pattern,
            candidate_literal: word_mode,
            raw,
            fold_word,
        })
    }

    fn matching_fields(
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
                let text = if self.raw {
                    Some(value.to_owned())
                } else {
                    render_text(value, format)
                };
                if text.is_some_and(|text| {
                    if self.fold_word {
                        self.regex.is_match(&text.case_fold().collect::<String>())
                    } else {
                        self.regex.is_match(&text)
                    }
                }) {
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

    fn matches_malformed(&self, bytes: &[u8]) -> bool {
        let text = crate::text::malformed_search_text(bytes);
        let text = if self.raw {
            text
        } else {
            render_text(&text, crate::TextFormat::Plain).expect("plain text renders")
        };
        if self.fold_word {
            self.regex.is_match(&text.case_fold().collect::<String>())
        } else {
            self.regex.is_match(&text)
        }
    }

    /// Normal postings retain scalar values but do not normalize whitespace.
    /// Broad or whitespace-sensitive patterns therefore scan all file slots.
    fn regular_candidate_pattern(&self) -> Option<&str> {
        let pattern = self.candidate_pattern.as_deref()?;
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


impl Repository {
    /// Build a collection's separate full-text index if it is not warm yet.
    pub fn warm_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        if self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists")
            .ensure_text_index()?
        {
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Warm all collections' full-text indexes.
    pub fn warm_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.warm_text_index(name)?)))
            .collect()
    }

    /// Refresh a collection and rebuild its separate full-text index if eligible.
    pub fn rebuild_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        picker.rebuild_text_index()?;
        if picker.text_index_bytes().is_some() {
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Rebuild every collection's full-text index.
    pub fn rebuild_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.rebuild_text_index(name)?)))
            .collect()
    }

    /// Search rendered words, rendered regex, or raw-markup regex in ID order.
    pub fn search_text(&mut self, request: &TextQuery) -> Result<TextQueryResult> {
        self.search_text_inner(request, false, &mut |_, _| Ok(()))
            .map(|(result, _)| result)
    }

    /// Emit page hits as they are verified. Hits remain provisional until the
    /// final summary; a later source error invalidates earlier callback data.
    pub fn search_text_stream(
        &mut self,
        request: &TextQuery,
        mut on_hit: impl FnMut(usize, &TextHit) -> Result<()>,
    ) -> Result<TextQueryStreamSummary> {
        let (result, returned) = self.search_text_inner(request, true, &mut on_hit)?;
        Ok(TextQueryStreamSummary {
            returned,
            total: result.total,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    fn search_text_inner(
        &mut self,
        request: &TextQuery,
        streaming: bool,
        on_hit: &mut dyn FnMut(usize, &TextHit) -> Result<()>,
    ) -> Result<(TextQueryResult, usize)> {
        let before = self.metrics.clone();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
        {
            return Err(Error::Request("invalid text pagination".into()));
        }
        let matcher = TextMatcher::compile(request)?;
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let requested_fields = request
            .fields
            .as_ref()
            .map(|fields| fields.iter().map(String::as_str).collect::<BTreeSet<_>>());
        if requested_fields.as_ref().is_some_and(BTreeSet::is_empty) {
            return Err(Error::Request("text field selection is empty".into()));
        }
        let mut matched = 0;
        let mut hits = Vec::new();
        let mut returned = 0;
        let mut used_regular_index = false;
        let mut used_text_index = false;
        for name in names {
            let c = self.collection(&name)?;
            let rules = TextRules::new(&c.fields);
            let source = Source::open_scoped(&c.path, c.recursive)?;
            let candidates = if self.options.cache == CacheMode::Disabled {
                source.inventory()?.into_iter().collect::<Vec<_>>()
            } else {
                self.warm_text_index(&name)?;
                let picker = &self.file_pickers[&name];
                let has_text_index = picker.text_index_bytes().is_some();
                used_text_index |= has_text_index;
                used_regular_index |= !has_text_index;
                picker.text_candidates(
                    if has_text_index {
                        matcher.candidate_pattern.as_deref()
                    } else {
                        matcher.regular_candidate_pattern()
                    },
                    matcher.raw,
                    matcher.candidate_literal,
                )
            };
            self.metrics.fts_candidates += candidates.len() as u64;
            for batch in candidates.chunks(256) {
                let outcomes = batch
                    .par_iter()
                    .map(|(id, stamp)| -> Result<(Vec<String>, Metrics)> {
                        let mut metrics = Metrics::default();
                        let raw = Self::read_raw(&mut metrics, &source, id, Some(stamp))?;
                        let document = Self::parse_value(&mut metrics, &c, id, &raw.json, false);
                        let started = Instant::now();
                        let fields = if document.state.status == RecordStatus::IncorrectJson {
                            if requested_fields.is_none() && matcher.matches_malformed(&raw.json) {
                                vec!["$raw".into()]
                            } else {
                                Vec::new()
                            }
                        } else {
                            matcher
                                .matching_fields(
                                    &document.data,
                                    &rules,
                                    requested_fields.as_ref(),
                                    false,
                                )
                                .fields
                        };
                        metrics.query_nanos += nanos(started);
                        metrics.grep_scans += 1;
                        metrics.grep_bytes += stamp.size;
                        Ok((fields, metrics))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut page = Vec::new();
                for ((id, stamp), (fields, metrics)) in batch.iter().zip(outcomes) {
                    self.metrics.add_from(&metrics);
                    if !fields.is_empty() {
                        if matched >= request.offset && returned + page.len() < request.limit {
                            page.push((id.clone(), stamp.clone(), fields));
                        }
                        matched += 1;
                    }
                }
                for (id, stamp, matched_fields) in page {
                    let raw = Self::read_raw(&mut self.metrics, &source, &id, Some(&stamp))?;
                    let document = Self::parse_value(&mut self.metrics, &c, &id, &raw.json, true);
                    let malformed = document.state.status == RecordStatus::IncorrectJson;
                    let verified = if malformed {
                        if requested_fields.is_none() && matcher.matches_malformed(&raw.json) {
                            TextMatches {
                                fields: vec!["$raw".into()],
                                matched_paths: vec!["$raw".into()],
                            }
                        } else {
                            TextMatches {
                                fields: Vec::new(),
                                matched_paths: Vec::new(),
                            }
                        }
                    } else {
                        matcher.matching_fields(
                            &document.data,
                            &rules,
                            requested_fields.as_ref(),
                            true,
                        )
                    };
                    if verified.fields != matched_fields {
                        return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                    }
                    let hit = TextHit {
                        record: Record {
                            collection: name.clone(),
                            id: id.clone(),
                            path: self.path(&c, &id),
                            size_bytes: raw.stamp.size,
                            modified_unix_nanos: raw.stamp.modified.clone(),
                            hash: raw.hash,
                            data: document.data,
                            state: document.state,
                            raw: malformed.then_some(raw.json),
                        },
                        fields: matched_fields,
                        matched_paths: verified.matched_paths,
                    };
                    on_hit(returned + 1, &hit)?;
                    returned += 1;
                    if !streaming {
                        hits.push(hit);
                    }
                }
                if !request.count && returned >= request.limit {
                    break;
                }
            }
            source.finish()?;
            if !request.count && returned >= request.limit {
                break;
            }
        }
        Ok((
            TextQueryResult {
                hits,
                total: request.count.then_some(matched),
                metrics: self.metrics.since(&before),
                query_plan: vec![if self.options.cache == CacheMode::Disabled {
                    "direct full-text source scan; source verification".into()
                } else if used_regular_index && used_text_index {
                    "normal and full-text bigram candidates; source verification".into()
                } else if used_regular_index {
                    "normal bigram candidates; source text verification".into()
                } else if matcher.raw {
                    "raw markup full-text bigram candidates; source verification".into()
                } else {
                    "rendered full-text bigram candidates; source verification".into()
                }],
            },
            returned,
        ))
    }
}
