//! Bounded, query-scoped regular-expression compilation.

use std::sync::Arc;

use regex::{Regex, RegexBuilder};
use regex_syntax::ParserBuilder;
use regex_syntax::hir::{Hir, HirKind, literal::Extractor};
use roaring::RoaringBitmap;

use crate::bigram::{BigramIndex, BigramRequest};
use crate::query::{CaseSensitivity, RegexOptions};

const MAX_PATTERN_BYTES: usize = 4 * 1024;
const MAX_NESTING: u32 = 128;
const COMPILED_SIZE_LIMIT: usize = 8 * 1024 * 1024;
const DFA_SIZE_LIMIT: usize = 2 * 1024 * 1024;
const EXTRACTION_NODE_LIMIT: usize = 64;
const EXTRACTION_ALTERNATIVE_LIMIT: usize = 16;
const EXTRACTION_LITERAL_LIMIT: usize = 256;

#[cfg(test)]
thread_local! {
    static REGEX_COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn test_compile_count() -> usize {
    REGEX_COMPILES.with(std::cell::Cell::get)
}


/// Necessary record-level gram condition; every regex match satisfies it.
#[derive(Clone, Debug)]
pub(crate) enum RegexCondition {
    Literal(BigramRequest),
    All(Vec<Self>),
    Any(Vec<Self>),
}


impl RegexCondition {
    fn retained_heap_bytes(&self) -> usize {
        match self {
            Self::Literal(request) => request.retained_heap_bytes(),
            Self::All(parts) | Self::Any(parts) => parts
                .capacity()
                .saturating_mul(std::mem::size_of::<Self>())
                .saturating_add(
                    parts
                        .iter()
                        .map(Self::retained_heap_bytes)
                        .fold(0usize, usize::saturating_add),
                ),
        }
    }

    /// A missing child is unrestricted. Conjunction can retain its other
    /// restrictions; one unrestricted alternation branch lifts the whole OR.
    fn from_hir(hir: &Hir, remaining: &mut usize) -> Option<Self> {
        if *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        match hir.kind() {
            HirKind::Empty | HirKind::Look(_) => None,
            HirKind::Literal(literal) => Self::from_literal(&literal.0),
            HirKind::Class(_) => {
                let mut extractor = Extractor::new();
                extractor
                    .limit_class(EXTRACTION_ALTERNATIVE_LIMIT)
                    .limit_total(EXTRACTION_ALTERNATIVE_LIMIT)
                    .limit_literal_len(EXTRACTION_LITERAL_LIMIT);
                let literals = extractor.extract(hir);
                let alternatives = literals.literals()?;
                if alternatives.is_empty() || alternatives.len() > EXTRACTION_ALTERNATIVE_LIMIT {
                    return None;
                }
                let parts = alternatives
                    .iter()
                    .map(|literal| Self::from_literal(literal.as_bytes()))
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::Any(parts))
            }
            HirKind::Repetition(repetition) => (repetition.min > 0)
                .then(|| Self::from_hir(&repetition.sub, remaining))
                .flatten(),
            HirKind::Capture(capture) => Self::from_hir(&capture.sub, remaining),
            HirKind::Concat(parts) => {
                let useful: Vec<_> = parts
                    .iter()
                    .filter_map(|part| Self::from_hir(part, remaining))
                    .collect();
                match useful.len() {
                    0 => None,
                    1 => useful.into_iter().next(),
                    _ => Some(Self::All(useful)),
                }
            }
            HirKind::Alternation(parts) => {
                let branches = parts
                    .iter()
                    .map(|part| Self::from_hir(part, remaining))
                    .collect::<Option<Vec<_>>>()?;
                match branches.len() {
                    0 => None,
                    1 => branches.into_iter().next(),
                    _ => Some(Self::Any(branches)),
                }
            }
        }
    }

    fn from_literal(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > EXTRACTION_LITERAL_LIMIT {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        Some(Self::Literal(BigramRequest::new(text)))
    }

    pub fn candidates(&self, index: &BigramIndex) -> Option<RoaringBitmap> {
        match self {
            Self::Literal(request) => index.candidates_prepared(request),
            Self::All(parts) => {
                let mut result: Option<RoaringBitmap> = None;
                for part in parts {
                    if let Some(set) = part.candidates(index) {
                        if let Some(result) = &mut result {
                            *result &= set;
                        } else {
                            result = Some(set);
                        }
                    }
                }
                result
            }
            Self::Any(parts) => {
                let mut result = RoaringBitmap::new();
                for part in parts {
                    result |= part.candidates(index)?;
                }
                Some(result)
            }
        }
    }

    pub fn estimate(&self, index: &BigramIndex) -> Option<(u64, f64)> {
        match self {
            Self::Literal(request) => Some((
                index.estimate_prepared(request)?,
                request.gram_count(index.alphabet) as f64,
            )),
            Self::All(parts) => {
                let mut result: Option<(u64, f64)> = None;
                for part in parts {
                    if let Some((count, work)) = part.estimate(index) {
                        if let Some((current, total_work)) = &mut result {
                            *current = (*current).min(count);
                            *total_work += work;
                        } else {
                            result = Some((count, work));
                        }
                    }
                }
                result
            }
            Self::Any(parts) => {
                let mut count = 0u64;
                let mut work = 0.0;
                for part in parts {
                    let (branch_count, branch_work) = part.estimate(index)?;
                    count = count.saturating_add(branch_count);
                    work += branch_work;
                }
                Some((count.min(index.record_count as u64), work))
            }
        }
    }
}


/// A compiled Boolean matcher shared by the selected text representations.
#[derive(Clone, Debug)]
pub(crate) struct CompiledRegex {
    matcher: Arc<Regex>,
    condition: Option<RegexCondition>,
}


impl CompiledRegex {
    pub(crate) fn retained_local_heap_bytes(&self) -> usize {
        self.condition
            .as_ref()
            .map_or(0, RegexCondition::retained_heap_bytes)
    }

    pub(crate) fn retained_heap_bytes(&self) -> usize {
        // `regex::Regex` does not expose its compiled allocation size. Charge
        // the configured compile/cache ceilings, including the Arc allocation,
        // so a regex cannot silently occupy a small prepared-cache budget.
        std::mem::size_of::<Regex>()
            .saturating_add(2 * std::mem::size_of::<usize>())
            .saturating_add(COMPILED_SIZE_LIMIT)
            .saturating_add(DFA_SIZE_LIMIT)
            .saturating_add(self.matcher.as_str().len())
            .saturating_add(self.retained_local_heap_bytes())
    }

    pub fn compile(
        pattern: &str,
        options: RegexOptions,
        case_sensitivity: CaseSensitivity,
    ) -> Result<Self, String> {
        #[cfg(test)]
        REGEX_COMPILES.with(|count| count.set(count.get() + 1));
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err(format!(
                "regex pattern exceeds {MAX_PATTERN_BYTES} byte limit"
            ));
        }
        let matcher = RegexBuilder::new(pattern)
            .unicode(true)
            .case_insensitive(case_sensitivity == CaseSensitivity::Insensitive)
            .multi_line(options.multi_line)
            .dot_matches_new_line(options.dot_matches_new_line)
            .nest_limit(MAX_NESTING)
            .size_limit(COMPILED_SIZE_LIMIT)
            .dfa_size_limit(DFA_SIZE_LIMIT)
            .build()
            .map_err(|error| format!("invalid regex: {error}"))?;
        // The parser uses the matcher flags. Failure here only disables an
        // optimization; exact matching has already been compiled.
        let mut remaining = EXTRACTION_NODE_LIMIT;
        let condition = ParserBuilder::new()
            .unicode(true)
            .case_insensitive(case_sensitivity == CaseSensitivity::Insensitive)
            .multi_line(options.multi_line)
            .dot_matches_new_line(options.dot_matches_new_line)
            .nest_limit(MAX_NESTING)
            .build()
            .parse(pattern)
            .ok()
            .and_then(|hir| RegexCondition::from_hir(&hir, &mut remaining));
        Ok(Self {
            matcher: Arc::new(matcher),
            condition,
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.matcher.is_match(text)
    }

    pub(crate) fn find(&self, text: &str) -> Option<std::ops::Range<usize>> {
        self.matcher
            .find(text)
            .map(|found| found.start()..found.end())
    }

    pub fn condition(&self) -> Option<&RegexCondition> {
        self.condition.as_ref()
    }
}


#[cfg(test)]
#[path = "regex_query.test.rs"]
mod tests;
