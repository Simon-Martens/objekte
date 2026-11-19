#[derive(Debug)]
pub(crate) struct MappedFilterMatcher {
    pub(crate) operation: Arc<dyn BindingOperation>,
    pub(crate) literal: Value,
    pub(crate) operator: Operator,
    pub(crate) text_equivalent: bool,
}

impl PreparedFilterMatcher for MappedFilterMatcher {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + value_retained_heap_bytes(&self.literal)
    }

    fn matches(&self, value: Option<&Value>, _: Option<&[u8]>) -> Result<bool, String> {
        Ok(value
            .and_then(|value| self.operation.map_value(value))
            .as_ref()
            .is_some_and(|mapped| {
                compare_scalar(mapped, &self.literal, self.operator, Transform::Identity)
            }))
    }

    fn posting_compatible(&self) -> bool {
        true
    }

    fn bigram_compatible(&self) -> bool {
        self.text_equivalent
    }
}

#[derive(Debug)]
pub(crate) struct MappedTextMatcher {
    pub(crate) operation: Arc<dyn BindingOperation>,
    pub(crate) finder: Finder<'static>,
    pub(crate) case_sensitivity: CaseSensitivity,
}

impl MappedTextMatcher {
    pub(crate) fn new(
        operation: Arc<dyn BindingOperation>,
        literal: String,
        case_sensitivity: CaseSensitivity,
    ) -> Self {
        let finder = Finder::new(&literal).into_owned();
        Self {
            operation,
            finder,
            case_sensitivity,
        }
    }
}

impl PreparedTextMatcher for MappedTextMatcher {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.finder.needle().len()
    }

    fn matches(&self, text: &str) -> Result<bool, String> {
        let mapped = self.operation.map_text(text);
        Ok(match self.case_sensitivity {
            CaseSensitivity::Sensitive => self.finder.find(mapped.as_bytes()).is_some(),
            CaseSensitivity::Insensitive => {
                with_folded_candidate(&mapped, |folded| self.finder.find(folded).is_some())
            }
        })
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}

/// Regex over a selected registered text representation, one segment at a time.
#[derive(Debug)]
pub(crate) struct RegexTextMatcher {
    pub(crate) regex: CompiledRegex,
    pub(crate) transform: Transform,
    pub(crate) operation: Option<Arc<dyn BindingOperation>>,
}

impl PreparedTextMatcher for RegexTextMatcher {
    fn retained_bytes(&self) -> usize {
        // Every regex FTS binding also retains the original `fallback_regex`.
        // Its Arc<Regex> owns the compiled automaton once; these wrappers own
        // only their cloned candidate conditions.
        std::mem::size_of::<Self>() + self.regex.retained_local_heap_bytes()
    }

    fn matches(&self, text: &str) -> Result<bool, String> {
        if let Some(operation) = &self.operation {
            return Ok(self.regex.is_match(&operation.map_text(text)));
        }
        if self.transform == Transform::CaseFold {
            return Ok(with_folded_candidate_str(text, |folded| {
                self.regex.is_match(folded)
            }));
        }
        Ok(self.regex.is_match(text))
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}

use crate::query::binding::fts::value_retained_heap_bytes;

use super::*;
