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
    observations: Vec<ExecutionObservation>,
    phases: Vec<(usize, &'static str)>,
}

fn observation_log() -> &'static Mutex<HashMap<u64, Capture>> {
    static LOG: OnceLock<Mutex<HashMap<u64, Capture>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static CURRENT_CAPTURES: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
    static CURRENT_PHASES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ObservationPhaseGuard;

impl Drop for ObservationPhaseGuard {
    fn drop(&mut self) {
        CURRENT_PHASES.with(|phases| {
            phases.borrow_mut().pop();
        });
        let previous =
            CURRENT_PHASES.with(|phases| phases.borrow().last().copied().unwrap_or("search"));
        mark_observation_phase(previous);
    }
}

pub(crate) fn observation_phase(phase: &'static str) -> ObservationPhaseGuard {
    CURRENT_PHASES.with(|phases| phases.borrow_mut().push(phase));
    mark_observation_phase(phase);
    ObservationPhaseGuard
}

fn current_observation_phase() -> String {
    CURRENT_PHASES.with(|phases| {
        phases
            .borrow()
            .last()
            .copied()
            .unwrap_or("search")
            .to_owned()
    })
}

fn mark_observation_phase(phase: &'static str) {
    let capture_id = CURRENT_CAPTURES
        .with(|captures| captures.borrow().last().map(|(_, capture_id)| *capture_id));
    let Some(capture_id) = capture_id else {
        return;
    };
    let mut log = observation_log().lock().expect("observation log poisoned");
    if let Some(capture) = log.get_mut(&capture_id) {
        capture.phases.push((capture.observations.len(), phase));
    }
}

pub(crate) fn begin_observation_capture(instance_id: u64) {
    let capture_id = NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed);
    observation_log()
        .lock()
        .expect("observation log poisoned")
        .insert(
            capture_id,
            Capture {
                observations: Vec::new(),
                phases: Vec::new(),
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
    let mut observations = capture
        .map(|capture| (capture.observations, capture.phases))
        .unwrap_or_default();
    for (position, (start, phase)) in observations.1.iter().enumerate() {
        let end = observations
            .1
            .get(position + 1)
            .map_or(observations.0.len(), |(start, _)| *start);
        for observation in &mut observations.0[*start..end] {
            observation.phase = (*phase).to_owned();
        }
    }
    let observations = observations.0;
    observations
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
    if !has_local_capture(instance_id) {
        return;
    }
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
    if !has_local_capture(instance_id) {
        return;
    }
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            phase: current_observation_phase(),
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
    if !has_local_capture(instance_id) {
        return;
    }
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            phase: current_observation_phase(),
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
    if !has_local_capture(instance_id) {
        return;
    }
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            phase: current_observation_phase(),
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
    if !has_local_capture(instance_id) {
        return;
    }
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
    if !has_local_capture(instance_id) {
        return;
    }
    push_observation(
        instance_id,
        ExecutionObservation {
            node_id: 0,
            phase: current_observation_phase(),
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
    let Some(capture_id) = local_capture else {
        return;
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

fn has_local_capture(instance_id: u64) -> bool {
    if ACTIVE_CAPTURES.load(Ordering::Acquire) == 0 {
        return false;
    }
    CURRENT_CAPTURES.with(|captures| {
        captures
            .borrow()
            .iter()
            .rev()
            .any(|(capture_instance, _)| *capture_instance == instance_id || instance_id == 0)
    })
}

#[cfg(test)]
pub(crate) struct TestProviderGuard {
    instance_id: u64,
}

#[cfg(test)]
#[derive(Default)]
struct TestAccessRegistration {
    providers: Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
    suppress_native: bool,
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
pub(crate) fn install_test_access_providers_suppressing_native(
    instance_id: u64,
    providers: Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
) -> TestProviderGuard {
    install_test_access_configuration(instance_id, providers, true)
}

#[cfg(test)]
fn install_test_access_configuration(
    instance_id: u64,
    providers: Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>>,
    suppress_native: bool,
) -> TestProviderGuard {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .insert(
            instance_id,
            TestAccessRegistration {
                providers,
                suppress_native,
            },
        );
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
        .map(|registration| registration.providers.clone())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn test_native_access_suppressed(instance_id: u64) -> bool {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .get(&instance_id)
        .is_some_and(|registration| registration.suppress_native)
}

#[cfg(test)]
pub(crate) fn all_test_access_providers()
-> Vec<std::sync::Arc<dyn crate::query_access::contract::IndexAccessProvider>> {
    test_provider_registry()
        .lock()
        .expect("test provider registry poisoned")
        .values()
        .flat_map(|registration| registration.providers.iter().cloned())
        .collect()
}

#[cfg(test)]
fn test_provider_registry()
-> &'static std::sync::Mutex<std::collections::HashMap<u64, TestAccessRegistration>> {
    static PROVIDERS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<u64, TestAccessRegistration>>,
    > = std::sync::OnceLock::new();
    PROVIDERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_access::contract::{AccessId, OperationId, ProviderId};

    #[test]
    fn observation_capture_is_request_thread_local() {
        let instance = u64::MAX - 77;
        begin_observation_capture(instance);
        std::thread::spawn(move || {
            record_observation(
                instance,
                TermId::new(1),
                DomainId::new(2),
                AccessId {
                    provider: ProviderId(3),
                    operation: OperationId(4),
                },
                0,
                10,
                5,
                2,
            );
        })
        .join()
        .unwrap();
        record_observation(
            instance,
            TermId::new(5),
            DomainId::new(6),
            AccessId {
                provider: ProviderId(7),
                operation: OperationId(8),
            },
            0,
            10,
            5,
            2,
        );
        let observations = finish_observation_capture(instance);
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].term_id, 5);
    }
}
