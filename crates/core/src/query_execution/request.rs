//! Stable request scope shared by planning and execution.

use crate::prepared::PreparedQuery;

pub(crate) struct RequestExecution<'a> {
    pub(crate) prepared: &'a PreparedQuery,
}

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::query_access::contract::AccessId;
use crate::query_access::membership::DomainId;
use crate::query_diagnostics::trace::{ExecutionObservation, ObservationStatus};
use crate::query_predicate::TermId;

fn observation_log() -> &'static Mutex<HashMap<u64, Option<Vec<ExecutionObservation>>>> {
    static LOG: OnceLock<Mutex<HashMap<u64, Option<Vec<ExecutionObservation>>>>> =
        OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn begin_observation_capture(instance_id: u64) {
    observation_log()
        .lock()
        .expect("observation log poisoned")
        .insert(instance_id, Some(Vec::new()));
}

pub(crate) fn finish_observation_capture(instance_id: u64) -> Vec<ExecutionObservation> {
    observation_log()
        .lock()
        .expect("observation log poisoned")
        .remove(&instance_id)
        .flatten()
        .unwrap_or_default()
}

pub(crate) fn cancel_observation_capture(instance_id: u64) {
    observation_log()
        .lock()
        .expect("observation log poisoned")
        .remove(&instance_id);
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
    let mut log = observation_log().lock().expect("observation log poisoned");
    let Some(Some(observations)) = log.get_mut(&instance_id) else {
        return;
    };
    observations.push(ExecutionObservation {
        node_id: observations.len() as u32,
        term_id: term.get(),
        provider_id: operation.provider.0,
        operation_provider_id: operation.provider.0,
        operation_id: operation.operation.0,
        collection,
        domain: format!("{domain:?}"),
        input_records,
        candidate_records,
        proven_records,
        residual_verifications: 0,
        status: ObservationStatus::Executed,
    });
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
