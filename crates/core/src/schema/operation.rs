//! Public schema extension and prepared-matcher contracts.

use std::sync::Arc;

use crate::model::{LogicalType, Value};
use crate::query::{Literal, Operator};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transform {
    Identity,
    CaseFold,
}

/// Named field transformation used by indexing and scanning.
#[derive(Clone, Debug)]
pub struct CustomBinding {
    pub name: String,
    pub version: u32,
    pub transform: Transform,
}

/// Input required by one prepared exact field predicate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactInput {
    SelectedValue,
    OriginalRecordBytes,
}

/// Query-scoped exact evaluator prepared by a registered binding. A byte
/// evaluator owns missing-field and array semantics for its whole record;
/// value evaluators receive the ordinary field selection and quantifier rules.
pub trait PreparedFilterMatcher: Send + Sync + std::fmt::Debug {
    /// Estimated bytes owned by this prepared matcher, excluding its Arc.
    /// Extensions with heap-owned state should override this default.
    fn retained_bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }

    fn input(&self) -> ExactInput {
        ExactInput::SelectedValue
    }

    fn matches(&self, value: Option<&Value>, bytes: Option<&[u8]>) -> Result<bool, String>;
    fn posting_compatible(&self) -> bool {
        false
    }

    fn bigram_compatible(&self) -> bool {
        false
    }
}

/// Query-scoped literal substring evaluator over one reader-owned segment.
pub trait PreparedTextMatcher: Send + Sync + std::fmt::Debug {
    /// Estimated bytes owned by this prepared matcher, excluding its Arc.
    /// Extensions with heap-owned state should override this default.
    fn retained_bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }

    fn matches(&self, text: &str) -> Result<bool, String>;
    fn bigram_compatible(&self) -> bool {
        false
    }
}

/// One prepared named operation shared by FILTER, FTS, indexes, and scanners.
/// Implementations work at value or record boundaries, never per character.
pub trait BindingOperation: Send + Sync + std::fmt::Debug {
    fn accepts(&self, kind: &LogicalType, operator: Operator, multi: bool) -> bool;
    fn prepare_literal(&self, literal: &Literal, kind: &LogicalType) -> Result<Value, String>;
    fn map_value(&self, value: &Value) -> Option<Value>;
    fn map_text(&self, text: &str) -> String;

    /// Declare that `map_value` yields a stable scalar ordering representation.
    /// Opaque matchers and fallible record-dependent operations stay ineligible.
    fn sort_compatible(&self) -> bool {
        false
    }

    /// A dynamic field operand may be compared after both values are mapped.
    fn supports_dynamic_comparison(&self) -> bool {
        false
    }

    fn prepare_fts_literal(&self, text: &str) -> Result<String, String> {
        Ok(self.map_text(text))
    }

    /// Opt in only when `map_text` is the complete text representation on
    /// which a prepared regex may run without transforming regex syntax.
    fn regex_text_compatible(&self) -> bool {
        false
    }

    /// Return a prepared prefix only when mapping a source prefix preserves
    /// prefix matching in this operation's indexed string representation.
    fn prepare_posting_prefix(&self, _prefix: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    /// `None` selects the prepared mapped-value adapter. An override may
    /// inspect borrowed values or original record bytes and return errors.
    fn prepare_filter_matcher(
        &self,
        _literal: &Value,
        _operator: Operator,
    ) -> Result<Option<Arc<dyn PreparedFilterMatcher>>, String> {
        Ok(None)
    }

    /// `None` selects the prepared mapped-text substring adapter.
    fn prepare_text_matcher(
        &self,
        _literal: &str,
    ) -> Result<Option<Arc<dyn PreparedTextMatcher>>, String> {
        Ok(None)
    }

    /// `map_value` and `map_text` use identical string representations.
    fn value_text_equivalent(&self) -> bool {
        false
    }

    fn posting(&self) -> bool {
        false
    }

    fn bigram(&self) -> bool {
        false
    }

    fn complete_coverage(&self) -> bool {
        true
    }
}

/// Stable named binding and semantic/configuration version.
#[derive(Clone, Debug)]
pub struct RegisteredBinding {
    pub name: String,
    pub version: u32,
    pub operation: Arc<dyn BindingOperation>,
}
