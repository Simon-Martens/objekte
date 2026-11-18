# Core query execution corrections — validation evidence

Date: 18 November 2026

- Reviewed integration tip: `74e569f505b8059aba1befef517c0c58e1e87bc0`
- Reviewed implementation revision: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`
- Delivered implementation revision: `4baa7af067f74f5ffb5772ef641ff0994fc4173f`
- Delivery branch: `integration/core-query-execution-corrections`
- Delivery worktree:
  `/home/simon/source/objekte-integration-core-query-execution-corrections`

## Correctness and validation

The following commands passed on the delivered revision:

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The core test run passed 569 library tests, all integration tests, and the
documentation compile-fail test. Sixteen manual/profiling tests remain ignored
by their existing annotations.

Primary regressions and their sections:

| Section | Test / evidence |
| --- | --- |
| 1 | `gate_a_public_search_selects_either_provider_in_either_order` checks the eight-record candidate `{0,3,4}`, year `{1,3,4,6}`, intersection `{3,4}`, final `{3}`, both operation orders, evidence on/off, zero-limit count, ordered stream, native access, independent providers, and scan-only fallback. |
| 2 | The same Gate A test fails on the reviewed implementation with segmented records `{3,4}` when evidence is disabled; it now returns `{3}`. `same_span_occurrences_keep_distinct_plans_and_exact_semantics` checks an unindexed text residual with a selective exact term. |
| 3 | `same_span_occurrences_keep_distinct_plans_and_exact_semantics` uses one span across different fields, one span across different literals, AND, OR, evidence on/off, base and segmented execution. Membership domain tests cover repository, collection, revision, segment, and the former XOR collision. |
| 4 | Gate A proves the selected provider operations execute through public dispatch. `restriction_never_widens_the_original_domain_or_supplied_input` proves effective input `{1,3}` wins over original `{0,1,2,3}` and posting `{0,1,2}`, returning `{1}`. Selection tests cover deterministic cost reversal. |
| 5 | Empty unrestricted backing is canonical empty for all five membership representations. Delivery accounting no longer charges the moved page as both projected copy and delivered bytes. |
| 6 | `observation_capture_is_request_thread_local` proves another thread cannot contribute to an active capture. Q030 reports 2,251 candidates separately from 1,264 matches. Q141 labels ordered-page and exact-count observations. |
| 7 | Production callers use `query_plan/selection.rs` for order preference and native cost splitting, `query_results/ranking.rs` for winner accounting, predicate modules for source/residual projection, and access modules for provider offers and application. |

## Module ownership

| Previous coordinator responsibility | Production owner |
| --- | --- |
| FILTER/FTS order preference and native input-cost split | `query_plan/selection.rs` |
| Boolean legal-order metadata | `query_plan/boolean.rs` |
| Relation page fallback order | `query_plan/operations.rs` |
| Predicate preparation, residual programs, source projection | `query_predicate/{mod,scan}.rs` |
| Exact matcher substitution and evidence | `query_predicate/mod.rs` |
| Base native structured/text offers | `query_access/{native,text,registry}.rs` |
| Segment provider application and domain identity | `query_access/{native,text}.rs`, `executor.rs` adapter boundary |
| Source scanning, segment scanning, relation scanning | `query_execution/{source_scan,segment_scan,relations}.rs` |
| Ordered pages, cache replay, materialization | `query_results/{ordered,cache,materialize}.rs` |
| Ranking key and winner byte accounting | `query_results/ranking.rs` |
| Provisional page and replacement delivery | `query_results/delivery.rs` |
| Request-scoped observations and phase markers | `query_execution/request.rs` |
| Trace attachment and rendering data | `query_diagnostics/trace.rs` |

The coordinator still owns request sequencing and candidate composition, but
the replaceable cost, storage, predicate, ranking and delivery decisions have
explicit module owners and production callers.

## EXPLAIN and EXPLAIN ANALYZE

The plans for the delivered `ascii-off` run are in
`ascii-off-4baa7af0/query-plans.jsonl`.

Q030:

```text
candidates: 2251
matches: 1264
observation: phase=search, term=0, input=4928, candidate=2251, proven=0
```

Q141:

```text
ordered_page:
  term 0 year >= 1800: input=4928, candidate/proven=3821
  term 1 year < 1850:  input=3821, candidate/proven=3308
  term 2 FTS Almanach: input=4928, candidate=1627, proven=0
exact_count:
  term 2 FTS Almanach: input=4928, candidate=1627, proven=0
  term 0 year >= 1800: input=1627, candidate/proven=1151
  term 1 year < 1850:  input=1151, candidate/proven=982
```

The repeated physical work is real rather than inferred: ordered-page and
exact-count passes are separately labeled. Physical applications removed in
this correction: zero; this change makes the existing repeats attributable and
prevents them from being confused by guessed post-search strategy.

## Performance

Raw results:

- `ascii-off-4baa7af0/`
- `incremental-unicode-4baa7af0/`
- `q030-base-first/`, `q030-base-second/`
- `q030-final-first/`, `q030-final-second/`
- `q164-base-interleaved/`
- `q164-final-4baa-first/`, `q164-final-4baa-second/`

The reviewed baseline is the committed `final-f0df3ba6` evidence under
`docs/ai/generated/query-execution-refactoring/repairs/`.

Five-sample medians:

| Case | Reviewed median | Delivered median | Change |
| --- | ---: | ---: | ---: |
| Q001 | 15.456 ms | 14.792 ms | -4.3% |
| Q007 | 343.959 ms | 338.588 ms | -1.6% |
| Q030 | 2.986 ms | 3.002 ms | +0.5% |
| Q035 | 0.058 ms | 0.059 ms | +2.3% |
| Q084 | 0.056 ms | 0.058 ms | +2.3% |
| Q127 | 0.075 ms | 0.073 ms | -2.0% |
| Q141 | 4.500 ms | 4.654 ms | +3.4% |
| Q150 | 2.775 ms | 2.765 ms | -0.3% |
| Q163 | 2.151 ms | 2.136 ms | -0.7% |
| Q164 | 6.141 ms | 6.550 ms | +6.7% |
| Q181 | 34.658 ms | 34.865 ms | +0.6% |

Q030 exceeded both thresholds in the five-sample matrix but was neutral in
alternating 20-repetition checks: base medians were 2.800 ms and 2.922 ms;
delivered medians were 2.820 ms and 2.997 ms, respectively (+0.7% and +2.6%).
Candidate, parse, residual and result counters match.

Q164 also exceeded both thresholds in one five-sample matrix. Alternating
20-repetition checks did not reproduce a consistent slowdown: the delivered
first median was 6.123 ms versus base 6.606 ms (-7.3%), and the delivered
second median was 6.434 ms versus the interleaved base 6.606 ms (-2.6%).
Candidate, parse, residual and result counters match.

Incremental Unicode medians:

| Case/profile | Reviewed median | Delivered median |
| --- | ---: | ---: |
| Q030 unicode-off | 3.437 ms | 2.893 ms |
| Q030 unicode-warm | 19.062 µs | 18.823 µs |
| Q141 unicode-off | 4.293 ms | 4.298 ms |
| Q141 unicode-warm | 18.479 µs | 17.942 µs |
| Q163 unicode-off | 2.000 ms | 1.958 ms |
| Q163 unicode-warm | 16.706 µs | 16.245 µs |

All result digests and ordinary work counters match the reviewed run for the
existing benchmark cases.
