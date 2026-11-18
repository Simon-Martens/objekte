# Query execution review repairs

Date: 18 November 2026

- Reviewed repair base: `0738c7a959bbdac255e37c90a111bf70778697a5`
- Delivered repair implementation: `1afdf73852`
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

The FILTER result is now carried as an `Arc<RoaringBitmap>` through the FTS
handoff. Mutation uses `Arc::make_mut`, so unchanged input shares its owner and
new set algebra allocates only when the result genuinely changes.

Cache replay now maintains a running page-byte total and computes each appended
row's retained size once. Ordered cursors and ordered record fetching live in
`query_results/ordered.rs`; winner admission and `offer`/`offer_merged` live in
`query_results/ranking.rs`.

## R5 — module ownership

The segmented native adapter and its prepared-operation table live in
`query_access/segment.rs`. Request-owned observation collection lives in
`query_execution/request.rs`.

The requested high-priority extraction moved:

- `LifecycleOrderCursor` and ordered record fetching to
  `query_results/ordered.rs`.
- `MergedPageHit`, `offer`, `offer_merged` and page-entry policy to
  `query_results/ranking.rs`.
- Segmented executable offers and provider application to
  `query_access/segment.rs`.

The coordinator retains orchestration of the top-level lifecycle traversal;
the large legacy fallback kernel selection remains in `executor.rs` and is
reached through the segment adapter.

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
read, scanned records and result digests are identical. The performance item is
closed by owner decision: it is not treated as a functional blocker, and no
unverified cause is attributed to the repair.

## Status

The code and evidence are pushed for review. R1, the measured diagnostic
repairs, the compound native ordering fixture, the requested ownership moves
and the bitmap/cache ownership repairs are implemented. Q164 is closed by owner
decision despite noisy timing.
