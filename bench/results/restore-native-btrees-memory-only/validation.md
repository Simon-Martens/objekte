# Validation commands and outcomes

Final candidate code: `2a2c12189d5a4a48ff644d6cf744c6a837b7d3d5`
(report/ledger and evidence commits follow on the same branch).

```sh
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
git diff --check
```

Outcomes on the final code:

| Check | Outcome |
| --- | --- |
| Workspace tests | core 515 passed / 15 ignored; CLI 81, process 33 + 1 ignored, removed-flags 1; desktop 160 / 1 ignored; all integration and doc tests passed. |
| Workspace all-targets check | Passed. |
| Clippy | 221 actual warnings, no errors; no new diagnostic category versus START after normalizing variant-size messages. |
| Detailed-profiling core check | Passed. |
| `git diff --check` | Passed. |
| Focused review tests | `status_estimate_reuses_cached_native_accounting`; `native_accounting_grows_with_postings_and_survives_retirement`; `metadata_fast_paths_and_fallbacks_match_the_reference_evaluator`; `record_range_scheduling_boundary_preserves_results_and_cancellation`; generated-namespace discovery/mutation/alias tests; legacy storage preference-field test. |

Retained limitations and omissions:

- The full 148k-record native GUI debug build was not used for the idle polling
  measurement because debug construction is much slower than the release
  benchmark; a configured 200k-record/100-file JSONL fixture was used instead.
  The GUI idle CPU delta did not isolate the polling work because repaint
  dominates. The no-traversal guarantee is proven by the instrumented
  regression test and the cached aggregation code.
- `query-plans.jsonl` was omitted from the evidence package; result tables,
  digests, manifests, raw repetition samples and reports are retained.
- Historical compacted-base and verified-update limitations remain open and are
  recorded in the task report.
