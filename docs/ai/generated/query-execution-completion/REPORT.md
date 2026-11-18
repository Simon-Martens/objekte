# Query execution completion — closing evidence

Date: 18 November 2026

- Starting revision: `4dd349ade31eae4edc2838fc7860c91f6368f2e7`
- Delivered implementation revision: `4b8fb2207b`
- Integration branch: `integration/query-execution-refactoring`
- Implementation commits:
  - `67ec75be40` — executable segmented offers and request-owned traces
  - `4b8fb2207b` — remove ordinary-search observation overhead

## Acceptance outcomes

### 1. Index selection — passed

`native_segment_selects_posting_or_text_first_for_4096_records` builds one
published JSONL segment with 4,096 logical records, native
`year = 1800` postings and a field-specific title gram index.

Filter first:

```text
year = 1800:
  input 4096
  candidates 8
  proven 8
  access posting

FTS title "rare":
  input 8
  candidates 5
  proven 0

exact verification:
  IDs 0, 1536, 3072
```

The test covers public search, zero-limit exact count, sorted page and ordered
stream, with evidence on/off and reference-evaluator comparisons.

Reverse distribution:

```text
FTS title "rxr":
  input 4096
  candidates 2
  proven 0

note = "keep":
  input 2
  candidates 2
  proven 2
```

The selected provider application runs on the actual survivor input. The
segment adapter now prepares native posting operations, advertises their
estimated output and lookup cost, executes the selected operation and carries
a source/native fallback with semantics distinct from an exact posting.

### 2. Result ownership and limits — passed for measured paths

- Exact posting results use one shared owner from provider restriction through
  segment bounds and proof; `shared_bitmap_from_slot_input` moves owned and
  shared results instead of cloning them.
- Provisional delivery no longer charges the page as both retained and newly
  moved live bytes.
- `replacements_correct_late_better_sort_key_and_preserve_payloads` measures
  the actual peak at one page plus keys and scratch. A budget equal to the
  measured peak succeeds; one byte below fails with the existing resource
  error.

The remaining borrowed-result case is covered by the existing exact-owner and
membership tests. No candidate bitmap is rebuilt by EXPLAIN.

### 3. Execution diagnostics — passed for Q030/Q141

Final plans are under `final-4b8fb220/ascii-off/query-plans.jsonl`.

Q030:

```text
ordinary candidates: 2251
trace candidates:    2251
final matches:       1264
phase:               search
```

Q141:

```text
ordinary candidates: 982
trace candidates:    982
final matches:       823
```

The trace retains the real physical observations:

```text
year >= 1800: input 4928, candidate 3821, proven 3821
year < 1850:  input 3821, candidate 3308, proven 3308
FTS Almanach: input 4928, candidate 1627, proven 0
```

The follow-up pass repeats the same operations on narrowed inputs:

```text
FTS Almanach: input 4928, candidate 1627, proven 0
year >= 1800: input 1627, candidate 1151, proven 1151
year < 1850:  input 1151, candidate 982, proven 982
```

Observation collection is request-owned through `ExecutionContext`; ordinary
search bypasses observation construction before formatting a domain or
allocating term IDs. Worker observations are merged by the request boundary.

### 4. Module ownership — partially complete

The native segmented provider moved from `executor.rs` to
`query_access/segment.rs`. It now owns prepared operation handles, posting
proposal/application, source fallback and observation attachment.

| Responsibility | Production owner |
| --- | --- |
| Native segmented offers and application | `query_access/segment.rs` |
| Base collection/text access | `query_access/{native,text}.rs` |
| Offer cost selection and text-first choice | `query_plan/selection.rs` |
| Prepared leaf/Boolean identity | `prepared.rs`, `query_predicate/mod.rs` |
| Request trace collection | `query_execution/request.rs` |
| Trace attachment/rendering | `query_diagnostics/trace.rs` |
| Provisional delivery and byte checks | `query_results/delivery.rs`, `streaming.rs` |
| Ranking accounting | `query_results/ranking.rs` |

This is not yet a complete fulfillment of the ownership map:
`query_execution/runner.rs` is 6,926 lines and still owns lifecycle ordered
paging/stream cursor coordination; `executor.rs` is 3,838 lines and retains the
legacy native fallback kernels used by `query_access/segment.rs`. Changing an
ordered traversal still requires editing the runner, so the criterion that the
runner contain no ordered traversal implementation remains open.

## Validation

The following passed on `4b8fb2207b`:

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The core run passed 570 library tests, all integration tests, and the doc test.
Sixteen existing manual/profiling tests remain ignored.

## Performance

Raw full and paused evidence:

- `final-4b8fb220/ascii-off/`
- `final-4b8fb220/incremental-unicode/`
- `final-4b8fb220/q030-q164-focused/`
- `final-4b8fb220/q164-focused-2/`

Q030 focused median was 2.633 ms. Q141 focused median was 4.413 ms versus the
reviewed baseline's 4.293 ms (+2.8%, below both escalation thresholds).

Q164 was noisy and remained slower in focused runs: medians were 7.577 ms and
7.017 ms versus the reviewed five-sample baseline median of 6.141 ms. Result
digests, candidate counts and ordinary work counters are unchanged. The
performance difference is not explained by an additional candidate pass in
the final trace, but its cause has not been established. This performance
investigation and the remaining module-ownership extraction are unfinished.

## Status

The implementation is pushed on `integration/query-execution-refactoring` for
review. The assignment should remain in `docs/ai/tasks/` until the ownership
and Q164 performance follow-ups are completed.
