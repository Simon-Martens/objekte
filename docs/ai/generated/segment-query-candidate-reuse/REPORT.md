# Segment query candidate reuse

Date: 18 November 2026

- Base commit: `1bafff6fa53be3215f0b48731550cce569f699c3`
- Implementation commit: `98b5d3a2`
- Delivery branch: `integration/segment-query-candidate-reuse`

## Implemented

### FILTER ownership

The ordered and ordinary segmented candidate path no longer calls
`Arc::make_mut` while segment bounds still retain the shared bitmap.

- A wholly logical segment forwards the existing candidate owner.
- A known logical-only result is forwarded unchanged.
- A mixed segment checks containment before intersecting once.
- The structured posting's possible/proven owner remains shared.

The public 4,096-record fixture records the real provider output pointer and
the FTS provider input pointer. The test requires them to be the same live
allocation. It retains the sequence `4096 -> 8 -> 5 -> 3` and IDs
`0, 1536, 3072`.

### Restricted gram lookup

`BigramIndex::candidates_prepared_restricted` intersects the most selective
gram with the supplied effective input before intersecting additional gram
columns. `NativeSegmentTextProvider` now passes its effective input directly
to that kernel and forwards its result without a second downstream
intersection.

Literal probes union only restricted results. Regex candidates are
conservatively intersected with the effective input. Missing usable gram
coverage forwards the incoming candidates for residual verification.

`prepared_gram_candidates_apply_input_before_narrowing` checks that an input
`{0,2,3}` never expands and remains the source boundary for gram narrowing.

### Ordered FTS input

The lifecycle ordered traversal now passes the FILTER survivors into FTS
instead of restarting from the active segment domain.

Q141 final trace:

```text
phase=ordered_page
year >= 1800: input 4928, output 3821, proven 3821
year < 1850:  input 3821, output 3308, proven 3308
FTS Almanach: input 3308, output 982, proven 0
```

Ordinary candidates remain 982 and matches remain 823. The fallback exact
count is labeled `search` rather than the ordered attempt.

## Validation

Passed on `98b5d3a2`:

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The core suite passed 572 library tests, all integration tests and the doc
test. Sixteen existing manual/profiling tests remain ignored.

Raw evidence:

- `q141-base/`
- `q141-final/`
- `ascii-off/`
- `incremental-unicode/`

Q141 five-repetition Unicode-off medians:

```text
base  6.456 ms
final 4.310 ms
```

The final ASCII Q141 median was 4.748 ms. All result digests and ordinary work
counters match the base.

## Remaining acceptance gap

The ordered path now consumes narrowed FILTER membership, which removes the
full-domain FTS restart. Request-local candidate state is not yet carried from
a late ordered decline or completed ordered page into ordinary fallback/count,
so an exact-count fallback can still reconstruct equivalent candidates.

Because this remaining reuse is part of requirement 2, the task should remain
open until the late-decline and page/count candidate-state transfer is
implemented and tested.
