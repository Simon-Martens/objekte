# Validation commands and outcomes

Final candidate code: `7ec29746c81cae8bab958c9688c8008fdd5d791f`
(evidence/report commits follow on the same branch).

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
| Workspace tests | core 516 passed / 15 ignored; CLI 81, process 33 + 1 ignored, removed-flags 1; desktop 160 / 1 ignored; all integration and doc tests passed. |
| Workspace all-targets check | Passed. |
| Clippy | 221 actual warnings, no errors; no new diagnostic category versus START after normalizing variant-size messages. |
| Detailed-profiling core check | Passed. |
| `git diff --check` | Passed. |
| Focused review tests | `status_estimate_reuses_cached_native_accounting`; `native_accounting_grows_with_postings_and_survives_retirement`; `metadata_fast_paths_and_fallbacks_match_the_reference_evaluator`; `record_range_scheduling_boundary_preserves_results_and_cancellation`; `canonical_alias_source_roots_are_rejected_for_direct_selection`; `followed_discovery_ignores_reserved_dangling_and_looping_links`; `stream_discovery_ignores_reserved_dangling_and_looping_links`; generated-namespace discovery/mutation/alias tests; legacy storage preference-field test. |

The retained query campaign was launched from the START master checkout and the
final integration checkout against the same immutable corpus; the manifests
therefore record `66b932f31f922a13062a61658e843e3033b3aff4` and
`7ec29746c81cae8bab958c9688c8008fdd5d791f`, and the executable hashes match the
binary hashes documented in `README.md`.

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
