use super::RepositoryOptions;

#[test]
fn exported_default_prepared_cache_budget_is_sixteen_mib() {
    let options = RepositoryOptions::default();
    assert_eq!(options.prepared_cache_bytes, 16 * 1024 * 1024);
}
