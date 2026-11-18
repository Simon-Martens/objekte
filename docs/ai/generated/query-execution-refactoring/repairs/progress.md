# Query execution repair progress

Task: `core-query-execution-repair`

Reviewed repair tip: `20e17c5be0d1f6c227503d5681c2bf7c76ca2808`

This record covers the repair work completed through commit
`bf32a30e25`. The repair specification is **not complete**; pending R3/R5–R7
items are listed below rather than represented as passing.

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
- R5: provider-backed skipped, cache-hit, relation and multi-domain
  observations are now real, but fused FILTER/FTS physical-node identity is not
  yet represented completely.
- R6: native direct producers stamp the pinned revision without delivery
  copies, and provisional delivery moves winners instead of cloning them.
  Exact segment bounds still retain duplicate `may`/`must` owners, and cache
  evidence construction still needs the compact-reference pass.
- R7: source scanning, segment scanning, relations, ordering, ranking,
  materialization and delivery have implementation owners and production
  callers. Cache replay and remaining orchestration/copy sites still need
  extraction from the 8,435-line coordinator.
- R8: the revised core validation and paired benchmark pass on
  `bf32a30e25`; it must be rerun after the remaining R3/R5–R7 corrections before
  the repair task can move to `done/`.
