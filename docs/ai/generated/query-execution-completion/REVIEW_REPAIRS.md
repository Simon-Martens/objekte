# Query execution review repairs

Date: 18 November 2026

- Reviewed repair base: `0738c7a959bbdac255e37c90a111bf70778697a5`
- Delivered repair implementation: `338cbbfae7`
- Original comparison revision: `4dd349ade31eae4edc2838fc7860c91f6368f2e7`
- Branch: `integration/query-execution-refactoring`

## R1 — effective logical range restriction

Implemented:

- Segment posting kernels now receive `effective_input & logical_domain`, not
  mixed logical/fallback input with a full logical coverage domain.
- Scalar range shortcuts therefore cannot skip their mask merely because the
  mixed input and logical domain have equal cardinality.
- Empty numeric proofs canonicalize to `Proven::None`; fallback slots remain
  possible but cannot become proven.

Regression:

`numeric_range_posting_excludes_retired_logical_slots_before_fallback`

```text
logical slots  {0, 1}
fallback slot  {2}
effective      {0, 2}
predicate      value >= 20
possible       {2}
proven         {}
```

Passed in debug and release profiles.

## R2 — execution diagnostics

Q181 now reports its ordinary source candidate contribution separately from
relation-target work:

```text
ordinary candidates: 4928
trace candidates:    4928
final matches:       6
```

Observation:

```text
phase=search
term=source work
input=4928
candidates=4928
kind=source_candidates
```

Q030 remains `2251` candidates / `1264` matches.

Q141 remains `982` candidates / `823` matches. Its six physical observations
retain inputs `4928 -> 3821 -> 3308` and the follow-up narrowed pass
`4928 FTS -> 1627 -> 1151 -> 982`.

The final sample is under `review-repairs-338cbbfa/ascii-off/query-plans.jsonl`.

## R3 — executable indexes and compound filters

The native segment adapter now offers posting and native fallback/sorted-column
alternatives together instead of suppressing posting offers when a sort column
exists. Posting output estimates remain clamped to effective input.

`native_segment_selects_posting_or_text_first_for_4096_records` retains:

- 4,096-year posting → 8 survivors → 5 gram candidates → IDs `0,1536,3072`.
- Rare FTS → 2 survivors → structured input of 2.
- Compound `year > 0 && year = 1800`; the second operation receives eight
  survivors and proves all eight.
- Public search, exact count, sorted page, ordered stream, evidence and
  reference comparisons.

## R4 — ownership and live bytes

- Native posting restrictions move owned results, preserve shared Arc owners,
  and canonicalize empty proofs.
- Provisional delivery no longer charges moved page bytes twice.
- A measured live-page budget succeeds exactly at the recorded peak; one byte
  below returns the existing resource error:

`replacements_correct_late_better_sort_key_and_preserve_payloads`.

Remaining copies:

- The lifecycle ordered collector still copies emitted hits.
- Repository/lifecycle stream wrappers still copy a batch when a row requires
  diagnostic remapping.
- `search_segmented_impl` still clones a FILTER bitmap before handing it to
  FTS.

These ownership sites remain open.

## R5 — module ownership

The segmented native adapter and its prepared-operation table live in
`query_access/segment.rs`. Request-owned observation collection lives in
`query_execution/request.rs`.

The larger requested extraction is not complete:

- Lifecycle ordered traversal and cursors remain in `runner.rs`.
- The legacy fallback kernels remain in `executor.rs`.
- `offer`/`offer_merged` remain in the coordinator.

The coordinator is 6,926 lines after this repair; the ownership acceptance
criterion is still open.

## Validation

Passed on `338cbbfae7`:

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo test --release -p objekte-search-core numeric_range_posting_excludes_retired_logical_slots_before_fallback
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The core suite passed 571 library tests, all integration tests and the doc
test. Sixteen existing manual/profiling tests remain ignored.

## Q164 performance

Raw paired samples:

- `review-repairs-q164/base-first/`
- `review-repairs-q164/repair-first/`
- `review-repairs-q164/repair-second/`
- `review-repairs-q164/base-second/`

Twenty-repetition medians:

| Block | Revision | Median |
| --- | --- | ---: |
| Base first | `4dd349ad` | 7.173 ms |
| Repair first | `338cbbfa` | 7.937 ms |
| Repair second | `338cbbfa` | 12.056 ms |
| Base second | `4dd349ad` | 10.538 ms |

Both blocks show high host variance. Candidate counts, record parses, bytes
read, scanned records and result digests are identical. The repair is slower in
both paired blocks, but the absolute movement is not stable enough to identify
a code site from timing alone. No resolved performance cause is claimed.

## Status

The code and evidence are pushed for review. R1 and the measured diagnostic
repairs are implemented. R3 is implemented for native posting/fallback
competition and the compound fixture. R4 and R5 remain incomplete, and Q164
has an unresolved performance regression. The task remains open in
`docs/ai/tasks/`.
