//! Stable request scope shared by planning and execution.

use crate::prepared::PreparedQuery;

pub(crate) struct RequestExecution<'a> {
    pub(crate) prepared: &'a PreparedQuery,
}

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::query_access::contract::AccessId;
use crate::query_access::membership::DomainId;
use crate::query_diagnostics::trace::{ExecutionObservation, ObservationStatus};
use crate::query_predicate::TermId;

static ACTIVE_CAPTURES: AtomicUsize = AtomicUsize::new(0);
static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(1);

struct Capture {
    instance_id: u64,
    observations: Vec<ExecutionObservation>,
}

fn observation_log() -> &'static Mutex<HashMap<u64, Capture>> {
    static LOG: OnceLock<Mutex<HashMap<u64, Capture>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static CURRENT_CAPTURES: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn begin_observation_capture(instance_id: u64) {
    let capture_id = NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed);
    observation_log()
        .lock()
        .expect("observation log poisoned")
        .insert(
            capture_id,
            Capture {
                instance_id,
                observations: Vec::new(),
            },
        );
    CURRENT_CAPTURES.with(|captures| captures.borrow_mut().push((instance_id, capture_id)));
    ACTIVE_CAPTURES.fetch_add(1, Ordering::Release);
}

pub(crate) fn finish_observation_capture(instance_id: u64) -> Vec<ExecutionObservation> {
    let capture_id = CURRENT_CAPTURES.with(|captures| {
        let mut captures = captures.borrow_mut();
        captures
            .iter()
            .rposition(|(instance, _)| *instance == instance_id)
            .map(|position| captures.remove(position).1)
    });
    let capture = capture_id.and_then(|capture_id| {
        observation_log()
            .lock()
            .expect("observation log poisoned")
            .remove(&capture_id)
    });
    if capture.is_some() {
        ACTIVE_CAPTURES.fetch_sub(1, Ordering::Release);
    }
    capture
        .map(|capture| capture.observations)
        .unwrap_or_default()
}

pub(crate) fn cancel_observation_capture(instance_id: u64) {
    let capture_id = CURRENT_CAPTURES.with(|captures| {
        let mut captures = captures.borrow_mut();
        captures
            .iter()
            .rposition(|(instance, _)| *instance == instance_id)
            .map(|position| captures.remove(position).1)
    });
    let removed = capture_id.and_then(|capture_id| {
        observation_log()
            .lock()
            .expect("observation log poisoned")
            .remove(&capture_id)
    });
    if removed.is_some() {
        ACTIVE_CAPTURES.fetch_sub(1, Ordering::Release);
    }
}

pub(crate) fn record_observation(
    instance_id: u64,
    term: TermId,
    domain: DomainId,
    operation: AccessId,
    collection: u32,
    input_records: u64,
    candidate_records: u64,
    proven_records: u64,
) {
    record_observation_with_kind(
        instance_id,
        term,
        domain,
        operation,
        collection,
        input_records,
        candidate_records,
        proven_records,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_observation_with_kind(
    instance_id: u64,
    term: TermId,
    domain: DomainId,
    operation: AccessId,
    collection: u32,
    input_records: u64,
    candidate_records: u64,
    proven_records: u64,
    access_kind: Option<&str>,
) {
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            term_id: term.get(),
            term_ids: vec![term.get()],
            provider_id: operation.provider.0,
            operation_provider_id: operation.provider.0,
            operation_id: operation.operation.0,
            access_kind: access_kind.map(str::to_owned),
            collection,
            domain: format!("{domain:?}"),
            input_records,
            candidate_records,
            proven_records,
            residual_verifications: 0,
            status: ObservationStatus::Executed,
        },
    );
}

pub(crate) fn record_skipped_observation(
    instance_id: u64,
    term: TermId,
    domain: DomainId,
    collection: u32,
    input_records: u64,
) {
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            term_id: term.get(),
            term_ids: vec![term.get()],
            provider_id: u32::MAX,
            operation_provider_id: u32::MAX,
            operation_id: u32::MAX,
            access_kind: None,
            collection,
            domain: format!("{domain:?}"),
            input_records,
            candidate_records: 0,
            proven_records: 0,
            residual_verifications: 0,
            status: ObservationStatus::Skipped,
        },
    );
}

pub(crate) fn record_cache_hit_observation(
    instance_id: u64,
    domain: DomainId,
    collection: u32,
    input_records: u64,
    output_records: u64,
) {
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            term_id: u32::MAX,
            term_ids: Vec::new(),
            provider_id: u32::MAX,
            operation_provider_id: u32::MAX,
            operation_id: u32::MAX,
            access_kind: None,
            collection,
            domain: format!("{domain:?}"),
            input_records,
            candidate_records: output_records,
            proven_records: output_records,
            residual_verifications: 0,
            status: ObservationStatus::CacheHit,
        },
    );
}

pub(crate) fn record_fused_observation(
    instance_id: u64,
    terms: &[TermId],
    domain: DomainId,
    collection: u32,
    input_records: u64,
    candidate_records: u64,
    proven_records: u64,
    access_kind: &str,
) {
    record_fused_observation_with_residual(
        instance_id,
        terms,
        domain,
        collection,
        input_records,
        candidate_records,
        proven_records,
        0,
        access_kind,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_fused_observation_with_residual(
    instance_id: u64,
    terms: &[TermId],
    domain: DomainId,
    collection: u32,
    input_records: u64,
    candidate_records: u64,
    proven_records: u64,
    residual_verifications: u64,
    access_kind: &str,
) {
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            term_id: terms.first().map_or(u32::MAX, |term| term.get()),
            term_ids: terms.iter().map(|term| term.get()).collect(),
            provider_id: u32::MAX,
            operation_provider_id: u32::MAX,
            operation_id: u32::MAX,
            access_kind: Some(access_kind.into()),
            collection,
            domain: format!("{domain:?}"),
            input_records,
            candidate_records,
            proven_records,
            residual_verifications,
            status: ObservationStatus::Executed,
        },
    );
}

fn push_observation(instance_id: u64, mut observation: ExecutionObservation) {
    if ACTIVE_CAPTURES.load(Ordering::Acquire) == 0 {
        return;
    }
    let local_capture = CURRENT_CAPTURES.with(|captures| {
        captures
            .borrow()
            .iter()
            .rev()
            .find(|(capture_instance, _)| *capture_instance == instance_id || instance_id == 0)
            .map(|(_, capture_id)| *capture_id)
    });
    let capture_id = match local_capture {
        Some(capture_id) => capture_id,
        None => {
            let log = observation_log().lock().expect("observation log poisoned");
            let mut matches = log
                .iter()
                .filter(|(_, capture)| capture.instance_id == instance_id)
                .map(|(capture_id, _)| *capture_id);
            let Some(capture_id) = matches.next() else {
                return;
            };
            if matches.next().is_some() {
                // A worker without a request-local token must not merge into
                // an arbitrary concurrent query on the same repository.
                return;
            }
            capture_id
        }
    };
    let mut log = observation_log().lock().expect("observation log poisoned");
    let Some(observations) = log
        .get_mut(&capture_id)
        .map(|capture| &mut capture.observations)
    else {
        return;
    };
    observation.node_id = observations.len() as u32;
    observations.push(observation);
}

#[cfg(test)]
pub(crate) struct TestProviderGuard {
    instance_id: u64,
}

#[cfg(test)]
impl Drop for TestProviderGuard {
    fn drop(&mut self) {
        test_provider_registry()
            .lock()
            .expect("test provider registry poisoned")
            .remove(&self.instance_id);
    }
}

#[cfg(test)]
pub(crate) fn install_test_access_providers(
    instance_id: u64,
    providers: Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
) -> TestProviderGuard {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .insert(instance_id, providers);
    TestProviderGuard { instance_id }
}

#[cfg(test)]
pub(crate) fn test_access_providers(
    instance_id: u64,
) -> Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>> {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .get(&instance_id)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn all_test_access_providers()
-> Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>> {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .values()
        .flatten()
        .cloned()
        .collect()
}

#[cfg(test)]
fn test_provider_registry() -> &'static std::sync::Mutex<
    std::collections::HashMap<
        u64,
        Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
    >,
> {
    static PROVIDERS: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<
                u64,
                Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
            >,
        >,
    > = std::sync::OnceLock::new();
    PROVIDERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
