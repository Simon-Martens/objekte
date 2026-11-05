//! Compiled exact predicates and safe candidate constraints.

use super::*;


/// A necessary index condition. The engine supplies the existing postings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateConstraint {
    Unsupported,
    /// `representation` identifies the indexed text semantics, not merely a
    /// similar-looking rendering. An incompatible index must be ignored.
    BigramLiteral {
        literal: String,
        representation: String,
    },
    TypedRange {
        path: String,
        lower: Option<TypedValue>,
        upper: Option<TypedValue>,
    },
}


/// One compiled, immutable exact predicate. Mutable scratch belongs to the
/// worker; implementations must not retain the borrowed value or scratch.
pub trait CompiledOperator: Send + Sync {
    fn matches(&self, value: ScalarValue<'_>, scratch: &mut String) -> Result<bool>;
    /// Consume a representation that has already prepared visible text.
    fn matches_prepared(&self, value: ScalarValue<'_>, scratch: &mut String) -> Result<bool> {
        self.matches(value, scratch)
    }

    fn candidate(&self) -> CandidateConstraint {
        CandidateConstraint::Unsupported
    }

    /// Promise that evaluating a value excluded by `candidate` cannot fail.
    /// Frontend planning requires this before skipping exact evaluation;
    /// the default preserves error behavior by scanning all sources.
    fn can_skip_non_candidates(&self) -> bool {
        false
    }
}


/// Compile parameters once, then evaluate with the bound representation.
pub trait QueryOperator: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>>;

    /// Typed text binding for composed definitions. Legacy operators retain
    /// their original literal-contains contract until they opt into more.
    fn compile_text(&self, request: &TextCompileRequest) -> Result<Box<dyn CompiledOperator>> {
        if request.comparison != TextComparison::Contains
            || request.syntax != TextSyntax::Literal
            || request.case_sensitive.is_some()
        {
            return Err(Error::Request(format!(
                "operator {} does not support these text options",
                self.descriptor().identity
            )));
        }
        self.compile(&Value::String(request.pattern.clone()))
    }
}


pub(crate) struct BoundTextQuery {
    pub definition_identity: Option<String>,
    pub index_path: Option<String>,
    pub representation: Arc<dyn TextRepresentation>,
    pub preparation: TextPreparation,
    pub native_index_projection: Option<NativeTextProjection>,
    pub operator: Arc<dyn CompiledOperator>,
    pub identity: String,
    pub candidate: CandidateConstraint,
    pub cacheable: bool,
}


impl std::fmt::Debug for BoundTextQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundTextQuery")
            .field("identity", &self.identity)
            .finish()
    }
}


impl BoundTextQuery {
    /// A necessary condition alone cannot suppress errors from exact evaluation.
    pub(crate) fn allows_candidate_pruning(&self) -> bool {
        self.operator.can_skip_non_candidates()
    }
}
