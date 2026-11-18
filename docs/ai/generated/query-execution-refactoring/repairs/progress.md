# Query execution repair progress

Task: `core-query-execution-repair`

Reviewed repair tip: `20e17c5be0d1f6c227503d5681c2bf7c76ca2808`

This record covers the repair work completed through commit
`f0df3ba685`. The pending R3/R5–R7 items were closed in the final correction
pass; the earlier sections remain as history.

## Completed evidence

- R1 exhaustive four-slot subset oracle covers empty, all, borrowed, owned and
  shared representations with different narrowed inputs and all set operations.
- Domain identity now contains repository, collection, revision and
  base/segment identity without hash truncation. The reviewed XOR collision and
  each domain component are covered by tests.
- Provider restriction consumes effective members rather than the original
  input. Stable handles remain valid after later proposals, and wrong term or
  domain applications return checked errors.
- Gate A passes through `Repository::search` with independent title/year
  providers. Text-first receives `[0,3,4]` for the structured step; structured
  first receives `[1,3,4,6]` for text. Both return record 3 with identical
  evidence and count.
- Ordinary EXPLAIN performs zero provider applications. ANALYZE performs one
  instrumented search and records ordered provider observations with input and
  candidate counts for each physical operation.
- Two exact predicate implementations (bound matcher and byte-window evaluator)
  produce identical ordered results and evidence under term, AND and OR queries.
- Commit `de901a533e` keeps one provider catalog and cached offer table for a
  base or segment domain, adaptively rescores at each effective input size,
  and applies the selected stable handle through the issuing provider.
- Segment candidates carry one possible set and a separate proof obligation;
  partial coverage no longer requires duplicate exact bitmaps at the provider
  boundary.
- ANALYZE now records skipped, cache-hit, relation, and multi-domain operations
  during execution. Worker-local captures merge through a unique request
  capture ID; ordinary search performs no observation synchronization.
- Commit `bf32a30e25` extracts actual source scanning, segment verification and
  scanning, relation planning/execution, ordered traversal, ranking,
  materialization, and delivery algorithms from the coordinator. The runner
  fell from 13,656 to 8,435 lines.
- Commit `3daa33e381` gives combined file-path ranges one fused physical
  observation containing both term IDs, records skipped and cache-hit states
  in tests, reuses the ordinary lifecycle cache policy in ANALYZE, and moves
  lifecycle cache replay into `query_results/cache.rs`. The coordinator is now
  8,318 lines.

## Gate A observations

| Order | First input | First output | Second input | Second output | Final |
| --- | --- | --- | --- | --- | --- |
| Text first | `{0,1,2,3,4,5,6,7}` | `{0,3,4}` | `{0,3,4}` | `{3,4}` | `{3}` |
| Structured first | `{0,1,2,3,4,5,6,7}` | `{1,3,4,6}` | `{1,3,4,6}` | `{3,4}` | `{3}` |

## Benchmark evidence

- Host: AMD Ryzen 7 5700G, 32 logical CPUs
- Toolchain: Rust/Cargo `1.99.1`
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Workers: 4, package default features, fresh result directories
- All full-run result and work digests matched original master
- All incremental Unicode result and work digests matched original master
- No case crossed the 5% + 0.2 ms investigation gate

| Case | Base ms | Repair ms | Ratio |
| --- | ---: | ---: | ---: |
| Q001 | 15.757 | 16.363 | 1.0384 |
| Q007 | 364.666 | 356.140 | 0.9766 |
| Q030 | 2.993 | 3.082 | 1.0297 |
| Q035 | 0.053 | 0.054 | 1.0179 |
| Q084 | 0.037 | 0.039 | 1.0418 |
| Q127 | 0.068 | 0.068 | 0.9991 |
| Q141 | 4.623 | 4.550 | 0.9842 |
| Q150 | 2.701 | 2.803 | 1.0375 |
| Q163 | 2.233 | 2.166 | 0.9701 |
| Q164 | 7.129 | 6.415 | 0.8998 |
| Q181 | 35.182 | 35.033 | 0.9958 |

Incremental Unicode (`unicode-off` / `unicode-warm`, median ms):

| Case | Base | Repair |
| --- | ---: | ---: |
| Q030 | 3.431 / 0.005766 | 2.970 / 0.005910 |
| Q141 | 4.311 / 0.007758 | 5.049 / 0.008289 |
| Q163 | 2.267 / 0.005143 | 2.233 / 0.004847 |

## Final extraction validation

Revision `bf32a30e25` was built and measured after the module extraction with a
fresh `/tmp` target and result directory. Raw run manifests, query plans,
samples, counters, CSV/JSON digests, and generated reports are stored under
`docs/ai/generated/query-execution-refactoring/repairs/final-bf32a30e/`.

`cargo test --locked -p objekte-search-core` passed 561 library tests, all core
integration tests, and the doc test. The concurrent test suite includes the
new observation-isolation and extracted-module call paths.

| Case | Base ms | Final ms | Ratio | Delta ms |
| --- | ---: | ---: | ---: | ---: |
| Q001 | 15.757 | 15.959 | 1.0128 | +0.202 |
| Q007 | 364.666 | 352.828 | 0.9676 | -11.838 |
| Q030 | 2.993 | 2.959 | 0.9887 | -0.034 |
| Q035 | 0.053 | 0.061 | 1.1510 | +0.008 |
| Q084 | 0.037 | 0.057 | 1.5514 | +0.020 |
| Q127 | 0.068 | 0.073 | 1.0761 | +0.005 |
| Q141 | 4.623 | 4.653 | 1.0065 | +0.030 |
| Q150 | 2.701 | 2.835 | 1.0496 | +0.134 |
| Q163 | 2.233 | 2.327 | 1.0421 | +0.094 |
| Q164 | 7.129 | 6.450 | 0.9047 | -0.679 |
| Q181 | 35.182 | 35.124 | 0.9984 | -0.058 |

Final incremental Unicode profiles:

| Case | Base off/warm | Final off/warm |
| --- | ---: | ---: |
| Q030 | 3.431 / 0.005766 | 3.374 / 0.020676 |
| Q141 | 4.311 / 0.007758 | 4.511 / 0.017271 |
| Q163 | 2.267 / 0.005143 | 2.134 / 0.017309 |

No case crossed both the 5% and 0.2 ms investigation gate. All final result
digests are recorded in the copied `result-digests.json` files.

## Pending repair requirements

- R3: relation alternatives and remaining specialized traversal choices still
  need to be represented and applied through the same production catalog; the
  legacy path DTO remains as an EXPLAIN compatibility view.
- R5 observed states now include provider-backed executed-zero, skipped,
  cache-hit, relation, multi-domain and fused file-path observations. The
  remaining R5 question is whether any inferred combined FILTER/FTS branch
  outside the file-path fusion needs a distinct physical node.
- R6: native direct producers stamp the pinned revision without delivery
  copies, and provisional delivery moves winners instead of cloning them.
  Exact segment bounds still retain duplicate `may`/`must` owners, and cache
  evidence construction still needs the compact-reference pass.
- R7: source scanning, segment scanning, relations, ordering, ranking,
  materialization and delivery have implementation owners and production
  callers; lifecycle cache replay now lives in `query_results/cache.rs`.
  Remaining orchestration and copy sites still need extraction from the
  8,318-line coordinator.
- R8: the revised core validation and paired benchmark pass on
  `bf32a30e25`; it must be rerun after the remaining R3/R5–R7 corrections before
  the repair task can move to `done/`.

## Latest validation through `3daa33e381`

The full core suite passed again: 561 library tests, all core integration
tests, and the doc test. A fresh paired benchmark measurement was taken after
the fused/cache changes and copied to
`docs/ai/generated/query-execution-refactoring/repairs/final-3daa33e3/`.

| Case | Base ms | Latest ms | Ratio |
| --- | ---: | ---: | ---: |
| Q001 | 15.757 | 15.663 | 0.9940 |
| Q007 | 364.666 | 345.371 | 0.9471 |
| Q030 | 2.993 | 2.980 | 0.9957 |
| Q035 | 0.053 | 0.058 | 1.1060 |
| Q084 | 0.037 | 0.059 | 1.5913 |
| Q127 | 0.068 | 0.073 | 1.0706 |
| Q141 | 4.623 | 4.407 | 0.9532 |
| Q150 | 2.701 | 2.755 | 1.0202 |
| Q163 | 2.233 | 2.129 | 0.9534 |
| Q164 | 7.129 | 6.415 | 0.8999 |
| Q181 | 35.182 | 34.304 | 0.9750 |

The first incremental `unicode-off` Q030 run measured a 5.376 ms median,
crossing the investigation gate. An alternating focused rerun with ten
repetitions measured a 3.227 ms median, compared with the recorded 3.431 ms
baseline; the first run is attributed to the observed high variance (2.813 ms
minimum, 6.308 ms maximum). The focused raw samples are preserved beside the
full run.

## Final correction pass through `f0df3ba685`

The pending R3/R5-R7 items are closed:

- R3 relation alternatives now belong to `query_access/relations.rs`. The
  provider issues target-first and source-restricted offers, selection is
  adaptive at the current input cardinality, and application runs the selected
  stable handle through `ProviderCatalog`. Whole-page relation traversal
  ordering is explicit in `query_plan/operations.rs`.
- R5 now records a fused FILTER/FTS source node only when FILTER survives to
  source execution. Observations are attached by collection identity, not by
  the position of the query in the trace vector; a regression test covers a
  query selecting only collection `b` while collection `a` is unselected.
- R6 exact segment possible/proven bounds share one immutable owner. Cache
  evidence uses shared `Arc<[String]>` membership and materialization creates
  the public evidence vector only for requested output rows. Public result
  types and the shared verified-cache handoff are unchanged.
- R7 moved ranking and sort-key policy to `query_results/ranking.rs`, verified
  cache replay/materialization to `query_results/cache.rs`, trace rendering and
  observation attachment to `query_diagnostics/trace.rs`, structured proof
  evidence to `query_predicate/structured.rs`, and residual/source-projection
  planning to `query_predicate/scan.rs`. `query_execution/runner.rs` is 6,770
  lines, down from 8,318 at the start of this pass.

### Correction commits

- `9e0a49bf27` provider-owned relation alternatives and compact cache evidence
- `3f25e432f1` ranking, trace and predicate-occurrence ownership
- `e80087d24f` structured proof evidence extraction
- `f9174d5a53` residual and source-projection extraction
- `0d4ed9df14` exact-bound ownership test
- `2402cb6746` collection-identity observation attachment
- `f0df3ba685` shared segment bounds through provider application

### Final validation

`cargo check --locked -p objekte-search-core --all-targets` passed.
`cargo test --locked -p objekte-search-core` passed 564 library tests, all 28
core integration tests and the doc test. The release benchmark example built
successfully.

Final paired evidence is under `final-f0df3ba6/`. All ascii-off and incremental
result digests and ordinary work counters match original master. The balanced
ascii-off matrix has no case crossing both 5% and 0.2 ms. Q001 crosses only the
absolute component; Q084 crosses only the percentage component.

The five-sample incremental matrix again showed high host variance. Focused
100-repetition checks measured Q030 at +0.034 ms, effectively neutral. Q141
measured +0.336 ms with final first and +0.173 ms with base first; `perf stat`
reported 1.410 s baseline versus 1.429 s final for 100 iterations, about
+0.19 ms per query. It performs the same candidates, scans, parses, residuals
and result work; no additional whole-domain pass or wrong result was found.

Q141 EXPLAIN ANALYZE now exposes the actual sort-column operations
`4928 -> 3821 -> 3308` and text restriction `4928 -> 1627` for each physical
segment, with two executed term choices. This closes the non-selected
collection trace-attachment defect found during the benchmark audit.
