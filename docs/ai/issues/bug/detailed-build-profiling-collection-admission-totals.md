# Detailed build profiling omits collection admission totals

Category — bug
Originating task — `index-storage-backends`

The test `lifecycle::initial::tests::incremental_admission_totals_match_each_collection` builds two collections over the same directory with build profiling enabled. It expects two entries in `StartupStats::build_profile.admission_by_collection`. Both the clean task base `cf7253d048c9d6fe5d2a7e9b47441b2207cce932` and the storage implementation `c760904257c05cb50ccc50959021a78408378e12` return zero and fail the first assertion.

```sh
cargo test --locked -p objekte-search-core --lib \
  --features detailed-profiling,test-scan-instrumentation \
  incremental_admission_totals_match_each_collection
```

The fixture targets shared-file admission in `crates/core/src/lifecycle/initial/tests.rs`. The missing entries prevent reading per-collection instrumentation for this case. The default and scan-instrumented workspace suites completed. The storage task records the pre-existing failure and leaves instrumentation correction for separate review.

Trace the build profile from shared-file worker completion to `admission_by_collection`, then compare the counter updates with ordinary single-collection admission. The failing assertion establishes the missing entries. The precise stage that drops them is unverified.
