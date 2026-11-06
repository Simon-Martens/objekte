//! Frontend binding and engine-owned logical plans.

use super::*;


/// A frontend produces only a logical request; execution remains in core.
pub trait QueryFrontend: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn compile(&self, request: &Value, bindings: &ExtensionBindings<'_>) -> Result<LogicalPlan>;
}


/// Stable numeric handles are assigned after sorting identities at binding.
pub struct ExtensionBindings<'a> {
    pub(super) registry: &'a Extensions,
}


impl ExtensionBindings<'_> {
    pub fn extractor(&self, identity: &str) -> Option<u32> {
        self.registry
            .extractors
            .keys()
            .position(|name| name == identity)
            .and_then(|position| u32::try_from(position).ok())
    }

    pub fn operator(&self, identity: &str) -> Option<u32> {
        self.registry
            .operators
            .keys()
            .position(|name| name == identity)
            .and_then(|position| u32::try_from(position).ok())
    }

    pub fn representation(&self, identity: &str) -> Option<u32> {
        self.registry
            .representations
            .keys()
            .position(|name| name == identity)
            .and_then(|position| u32::try_from(position).ok())
    }
}


/// Boolean plan whose calls refer to immutable, bound extension definitions.
#[derive(Clone, Debug)]
pub enum LogicalPredicate {
    Call {
        operator: u32,
        representation: u32,
        path: String,
        parameters: Value,
    },
    /// Compare values emitted by a registered extractor. A missing field and
    /// an extractor that emits no values do not satisfy a value predicate.
    Derived {
        extractor: u32,
        predicate: crate::DerivedPredicate,
    },
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Box<Self>),
}


#[derive(Clone, Debug)]
pub struct LogicalPlan {
    pub collections: Vec<String>,
    pub predicate: LogicalPredicate,
    pub sort: Vec<crate::Sort>,
    pub columns: Vec<String>,
    pub offset: usize,
    pub limit: usize,
    pub count: bool,
}
