//! Stable request scope shared by planning and execution.

use crate::prepared::PreparedQuery;

pub(crate) struct RequestExecution<'a> {
    pub(crate) prepared: &'a PreparedQuery,
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
