# Complete hybrid-store failure validation coverage

- **Category:** feat
- **Originating tasks:** `hybrid-index-storage`, its repair specification, and the generation/validation plan
- **Observed revision:** `74cab22411` on `integration/hybrid-index-storage`
- **Code areas:** `crates/core/src/managed_store.rs`, `segment_format.rs`, `posting_dictionary.rs`, and lifecycle store-error adapters

## Observation and expected behavior

The generation store, validation stamps, and posting-proof path have passing workspace, corruption, concurrency and recovery tests. The [generation checkpoint](../../../../bench/results/hybrid-index-storage/maintenance-20261114/REPORT.md) explicitly records that its full failure-injection matrix was not completed: foreign-owned entries and every dictionary-specific corrupt-posting route were not individually exercised. The [repair checkpoint](../../../../bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md) also records that some lower-level helpers still return `String` errors rather than a typed store category.

The specifications call for deterministic error categories and preserved old generations across interrupted writes, manifest/catalog failures, ownership violations, malformed dictionaries/postings, and concurrent publication. Each route should be tested through the API that reports storage status, with corrupt bytes yielding errors rather than empty results or silent rebuilds. No data-loss or wrong-result defect is asserted here; the gap is unverified failure coverage and incomplete typed propagation at lower levels.

## Reproduction and impact

Review the test matrix in `docs/ai/done/hybrid-store-generation-validation-readpath-plan.md` Section 7 against `managed_store.test.rs` and posting/segment corruption tests. Run the specified fault injection on disposable stores, then compare returned `IndexError`/status categories and the retained previous generation. The existing test suite establishes many cases, but it is not a complete matrix. Untested branches can produce an inconsistent classification or recovery response, particularly under unusual filesystem ownership and crafted posting damage.

## Why deferred and possible approaches

The user approved closing the hybrid task while transferring outstanding work to issues. Completing every fault route and normalizing all lower-level error types would be a separate correctness audit. Add narrowly scoped fixtures for each missing route, propagate structured read/write causes at the relevant helper boundary, and verify CLI/TUI/desktop status mapping. Preserve the current checks and do not turn corruption into absence. This issue is for review; it does not authorize implementation.
