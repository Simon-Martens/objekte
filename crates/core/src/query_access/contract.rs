//! Capability, offer and execution contracts for index providers.

use std::fmt;

use roaring::RoaringBitmap;
use smallvec::SmallVec;

use crate::query_predicate::TermId;
use crate::reference::{BoundFts, BoundTerm};

use super::membership::{DomainId, SlotInput};

pub(crate) type AccessResult<T> = Result<T, AccessError>;
pub(crate) type OfferList = SmallVec<[AccessOffer; 8]>;

#[cfg(test)]
thread_local! {
    static TEST_APPLICATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_test_applications() {
    TEST_APPLICATIONS.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn test_applications() -> u64 {
    TEST_APPLICATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn note_test_application() {
    TEST_APPLICATIONS.with(|count| count.set(count.get() + 1));
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AccessError {
    CapabilityUnavailable,
    Cancelled,
    Corrupt(String),
    DomainMismatch,
    InvalidDomain(u64),
    Io(String),
    Resource(String),
    StaleHandle,
    Internal(String),
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityUnavailable => formatter.write_str("access capability unavailable"),
            Self::Cancelled => formatter.write_str("query cancelled"),
            Self::Corrupt(message) => write!(formatter, "corrupt index access: {message}"),
            Self::DomainMismatch => formatter.write_str("query slot domain mismatch"),
            Self::InvalidDomain(value) => write!(formatter, "invalid query domain {value}"),
            Self::Io(message) => write!(formatter, "index access I/O error: {message}"),
            Self::Resource(message) => write!(formatter, "index access resource limit: {message}"),
            Self::StaleHandle => formatter.write_str("stale index access handle"),
            Self::Internal(message) => write!(formatter, "index access internal error: {message}"),
        }
    }
}

impl std::error::Error for AccessError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProviderId(pub(crate) u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct OperationId(pub(crate) u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AccessId {
    pub(crate) provider: ProviderId,
    pub(crate) operation: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AccessKind {
    ExactMembership,
    CandidateMembership,
    Metadata,
    OrderedColumn,
    Relation,
    SourceScan,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Coverage {
    Complete,
    Partial { covered: u32 },
    CandidateOnly,
    Input,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ProofStrength {
    Exact,
    Candidate,
    None,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InputEstimate {
    pub(crate) domain_len: u64,
    pub(crate) input_len: u64,
    pub(crate) mean_record_bytes: f64,
    pub(crate) mean_records_per_file: f64,
    pub(crate) file_count: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct AccessOffer {
    id: AccessId,
    term: TermId,
    kind: AccessKind,
    coverage: Coverage,
    proof: ProofStrength,
    estimated_output: u64,
    startup_work: f64,
    input_work: f64,
    can_evidence: bool,
    can_order: bool,
}

impl AccessOffer {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: AccessId,
        term: TermId,
        kind: AccessKind,
        coverage: Coverage,
        proof: ProofStrength,
        estimated_output: u64,
        startup_work: f64,
        input_work: f64,
        can_evidence: bool,
        can_order: bool,
    ) -> Self {
        Self {
            id,
            term,
            kind,
            coverage,
            proof,
            estimated_output,
            startup_work,
            input_work,
            can_evidence,
            can_order,
        }
    }

    pub(crate) fn id(&self) -> AccessId {
        self.id
    }

    pub(crate) fn term(&self) -> TermId {
        self.term
    }

    pub(crate) fn kind(&self) -> AccessKind {
        self.kind
    }

    pub(crate) fn coverage(&self) -> Coverage {
        self.coverage
    }

    pub(crate) fn proof(&self) -> ProofStrength {
        self.proof
    }

    pub(crate) fn estimated_output(&self) -> u64 {
        self.estimated_output
    }

    pub(crate) fn estimated_work(&self, input_len: u64) -> f64 {
        self.startup_work + self.input_work * input_len.max(1) as f64
    }

    pub(crate) fn can_evidence(&self) -> bool {
        self.can_evidence
    }

    pub(crate) fn can_order(&self) -> bool {
        self.can_order
    }
}

/// Predicate-private translation passed to a provider. The optimizer never
/// receives this value or inspects its contents.
#[derive(Clone, Copy)]
pub(crate) enum AccessRequest<'a> {
    Structured(&'a BoundTerm),
    Text(&'a BoundFts),
}

pub(crate) struct AccessExecution<'a> {
    pub(crate) context: Option<&'a crate::streaming::ExecutionContext>,
}

impl AccessExecution<'_> {
    pub(crate) fn check(&self) -> AccessResult<()> {
        match self.context {
            Some(context) => context.check().map_err(|error| match error.kind {
                crate::streaming::ExecutionErrorKind::Cancelled => AccessError::Cancelled,
                crate::streaming::ExecutionErrorKind::Resource => {
                    AccessError::Resource(error.message)
                }
                _ => AccessError::Internal(error.message),
            }),
            None => Ok(()),
        }
    }
}

/// Storage providers expose only capability and cost information plus checked
/// bulk application. Concrete index types stay in adapter modules.
pub(crate) trait IndexAccessProvider {
    fn provider_id(&self) -> ProviderId;

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()>;

    fn apply<'a>(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<SlotInput<'a>>;
}

pub(crate) fn domain_with_slots<'a>(
    domain: DomainId,
    input: &'a RoaringBitmap,
    slots: super::membership::SlotSet<'a>,
) -> SlotInput<'a> {
    SlotInput::new(domain, input, slots)
}
