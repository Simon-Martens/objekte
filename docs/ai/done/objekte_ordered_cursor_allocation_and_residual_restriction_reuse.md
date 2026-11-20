# Objekte repair specification · Ordered cursor allocation and residual restriction reuse

Continue `integration/query-execution-path-repair` in `/home/simon/source/objekte-integration-query-execution-path-repair`. The reviewed tip is `9b63f56d5a4759e25d09ce1ea032f2dd8158dea4`, with implementation through `f7b536f5ee`. This is a continuation of the existing query execution assignment. Keep the original checkout on `master`, follow `docs/ai/README.md`, push the integration commits and leave the merge to the owner.

**Definition of done.** On a rejected key admission, the runner performs zero allocations or copies of the rejected key’s owned data. On a residual query that falls back, each indexed predicate is physically applied once per segment and effective input that needs it. Demonstrate both through the public APIs, and correct the one-file benchmark to contain the same 260 records as the 65-file case. The nested Boolean evidence repair passed review. Preserve its cold-stream and warm-cache regression.

## 1. Check the cursor budget before allocating the key

**Failure on the reviewed tip.** `push_lifecycle_order_cursor` calls `segment_rank_key` before checking the available bytes. That call allocates the parts vector, clones stored sort values, and creates owned collection and record IDs. The later resource error releases the newly constructed key, so the existing test sees the expected error and zero callbacks even though the copy already happened. Both initial heap population and successor insertion use this helper. [Cursor admission](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L3421-L3479) · [Owned key construction](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_results/ranking.rs#L347-L379) · [Existing boundary test](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/streaming/tests.rs#L1174-L1316)

**Implement the following sequence.**

1. Resolve the candidate using the pinned segment, slot and bound sort terms. Inspect the resident sort values by reference. The stored comparator already indexes `segment.sorting.columns` by the bound path, avoiding a formatted path string per key. Reuse that lookup and the existing sort semantics. [Borrowed column lookup](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_results/ranking.rs#L382-L415)
2. The smallest change that preserves the current cursor representation is a checked, allocation-free size calculation for the owned key the constructor will produce. Include the parts storage, collection and record-ID storage, and variable-length string collation bytes or ID terms. Use the same accounting convention as `rank_key_bytes`, with checked arithmetic. Account for the destination representation, including capacity where the constructor reserves it. Verify the calculation against a materialized key under a sufficient budget for empty, numeric and long variable-length parts. A second independent sizing formula that can drift from construction is a maintenance risk. [Existing byte accounting](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_results/ranking.rs#L752-L765)
3. Compare the required bytes with the allowance remaining after the current key, other heap entries, page and retained delivery identities. On overflow or insufficient allowance, return the typed resource error **before the first owned-key allocation**. On success, construct the key once and retain its charge through its last use. Keep the bounded source reader, replacement delivery and verified-cache admission on their existing budgets.
4. An equally valid implementation borrows resident sort values for each heap entry. If using it, preserve descending and composite sort order, tie-breaking by collection and ID, and segment-local slots. Charge any owned key required by cache insertion before constructing it. Preserve the existing cache rule that closes admission when a complete entry cannot fit. [Borrowed cursor comparator](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/ordered.rs#L68-L105) · [Cache admission](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L775-L800)

**Regression that distinguishes the fix.** Extend `ordered_successor_accounts_for_the_live_popped_key`. Put a test observation at the first owned allocation or copy for a particular segment and slot. Record attempts before any parts-vector allocation or stored-value clone. A counter at heap insertion, at pop, or after the budget check gives a false pass. Reset the observation after fixture setup and between runs.

| Run | Required observation |
| --- | --- |
| Initial key alone exceeds a legal buffer limit | Zero owned constructions for that slot. Resource error, zero callbacks, no partial cache entry. |
| First key fits, successor would exceed the overlap limit | The first key is admitted. Zero owned constructions for the rejected successor. Resource error at the expected progress. |
| Sufficient budget, cold accepting cache | Reference-equivalent rows, count and ordering. Record key sizes and peak buffering. Check the existing replacement and counted-tail cases. |

Keep the two long stored sort strings and residual source read in the fixture. Derive limits from observed sizes. The prior report’s 42,319-byte limit is machine-specific. If borrowing eliminates the historical overlap, demonstrate zero heap-key materializations and successful completion at a limit that covers the actual live state. Isolate source bytes or cache admission when testing the next real rejection boundary. Record the baseline copy attempt as well as the repaired observation.

## 2. Give the selected executor the restrictions already computed

**Failure on the reviewed tip.** With four workers, 65 candidate files and more than 256 candidates, the ordered probe builds per-segment bounds. It keeps a fully indexed OR on the ordered route, but declines a mixed query such as `a = true || note = 'needle'` when `note` needs source verification. The fallback receives bound sort and query information plus a Boolean. It computes the indexed `a` restriction again. The ordinary search path also has exact-page probes, and the general runner separately calculates exact OR branch membership for counters. Fixing only the public stream’s second call can leave other repeats. [Ordered bounds](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L3602-L3632) · [File-count decision](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L3732-L3765) · [Stream handoff](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L4629-L4642) · [Exact-page probe](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L1591-L1616) · [OR statistics](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L1662-L1729) · [General bounds](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L2219-L2244)

**Implementation boundaries.**

1. Give the current request one owner for prepared per-segment restrictions. Prefer passing the computed result from route selection to execution, or select the executor before applying the first posting when the same routing decision can be made cheaply. Carry completed work across both `IndexView::search` and `IndexView::search_stream`. Identify every physical apply in the failing trace, including exact-page attempts and statistics. Derive exact branch counts from known membership wherever the counter requires them. Preserve overlapping-branch union semantics.
2. Preserve both `may` (possible matches) and `must` (proven matches). Hand off complete bounds before the ordered path reduces them to a candidate bitmap. Keep any per-branch evidence needed by the selected route. Transfer ownership or share the existing bitmap allocation. Keep reuse scoped to the same pinned revision, collection, segment, bound expression and effective input slots. Equal cardinality does not establish equal input. A genuinely different FTS-first domain or a partially prepared segment must get the appropriate calculation. Keep this state private to one execution.
3. Preserve file-parallel residual verification where the existing scheduler admits it. The ordinary test currently uses `offset 25 + limit 50 = 75`; the ordinary parallel branch requires a page bound of at most 64 for multi-record file groups. It also requires `context.is_none()`, while the public stream’s general fallback supplies a context. Test ordinary worker dispatch with `offset 1 + limit 50 = 51`. For streaming, assert the actual fallback route and posting count while retaining its cancellation and resource checks. [Current test](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/tests.rs#L7315-L7353) · [Scheduler and worker dispatch](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/mod.rs#L2533-L2593)

**Measure physical work.** Instrument the actual posting application using the bound field/operator/value, segment identity, and effective slots. Use request-scoped test observations that cover the full call, including probes and worker activity where relevant. Separate planning offers, physical applications, and residual source reads. Segment-local numeric term IDs can differ between probes. The existing aggregate counter cannot identify which posting was repeated. Observe actual parallel batch dispatch on the ordinary residual fixture. A file count or an ordered-completion count of zero cannot establish that workers received jobs. [Provider execution](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_access/segment.rs#L250-L312) · [Current aggregate counter](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/restrictions.rs#L319-L344)

## 3. Use the same 260 records for correctness and timing

Update both `indexed_or_file_layout` and `indexed_or_layout_case`. Each currently writes four rows per file, so the one-file control returns only three rows after the offset. Generate records by global ordinal `i = 0..259` inclusive. Give each a stable intrinsic string ID and numeric stored sort value `i`. Write the same serialized rows in order either to one JSONL file or to 65 files of four. Declare booleans `a` and `b` indexed. Keep `note` residual. Use the same four workers, one segment large enough for all 65 files, disabled verified-result cache and locator output. Compare full hits with the reference within each layout. Across layouts compare stable IDs, order, fields and counts because file paths and byte offsets differ. [Test generator](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/src/query_execution/runner/tests.rs#L7163-L7194) · [Benchmark generator](https://github.com/Simon-Martens/objekte/blob/9b63f56d5a4759e25d09ce1ea032f2dd8158dea4/crates/core/examples/bench_query_execution_addendum.rs#L485-L516)

| Global row | a | b | note |
| --- | --- | --- | --- |
| `i % 4 == 0` | true | false | needle |
| `i % 4 == 1` | false | true | needle, except rows 1, 5 and 9 use needle-extra |
| `i % 4 == 2 or 3` | true | true | needle |

Run cold ordinary and streaming calls independently. Use `offset=1`, `limit=50`, `count=true` and ascending stored sort. Capture the baseline using the corrected fixture and the same probes before applying production changes.

| Query | Assertions |
| --- | --- |
| `a = true \|\| b = true`, evidence on, both layouts | Count 260, page of 50, full row and matched-field parity with reference. With the single segment, one physical application each for a and b. Zero source opens, bytes and parses. |
| `a = true \|\| note = 'needle'`, evidence off, 65 files | Count 257, page of 50, IDs 1, 5 and 9 excluded from the complete match set. The stream and ordinary call each apply the indexed a posting exactly once on the same input. Source verification runs. Ordinary search dispatches file jobs. |
| Existing nested Boolean evidence fixture | Cold streaming returns A with `["a","c"]`, count 3. The warm second page reports a verified-cache hit and the reference fields for B and C. |

Use the decimal string of the global ordinal as the value of `id`, with `IdentityPolicy::Intrinsic(vec!["id".into()])`. For the residual case, also collect one untimed full result at offset zero. Compare its IDs to the full reference result and explicitly assert that `"1"`, `"5"` and `"9"` are absent. Measure posting counts and worker dispatch separately on the specified 50-row page. If a route uses a genuinely narrower input, log both slot sets and why they differ. Provide actual baseline and final counts. Source inspection predicts duplication but does not establish its exact multiplicity.

## Verification and delivery

- Run the focused regressions first. Verify that the baseline fails the new allocation and posting assertions. Run the instrumentation on the reviewed code before changing production behavior, or apply the same test-only instrumentation to a detached baseline. Keep the reference evaluator independent and reset the verified-result cache before each cold run. A typed error alone or reference agreement alone leaves the respective defect undetected.
- Run three alternating baseline/candidate release pairs for the two corrected indexed-OR layouts with identical benchmark code and build settings. Time first nonempty callback and completion. Validate streamed order and evidence outside the timed section. Record row count, file count, result count, returned rows, worker count, cache state and source counters. The prior one-file numbers measure four rows. Label these new results as a separate comparison.
- Run `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace`, `cargo fmt --all -- --check` and `git diff --check`. Record the existing `crates/core/src/model/json.rs` formatting difference separately if present.
- Append a report to this spec under `docs/ai/done/` with the actual allocation attempts per slot, posting counts per input and entry point, observed execution routes, corrected timing table, test outcomes, and pushed code and report revisions. Mark an unmeasured criterion **unverified**. Keep the implementation within the existing core and benchmark surfaces.

## Implementation report

- Task identifier: `ordered-cursor-allocation-and-residual-restriction-reuse`
- Completed: 20 November 2026
- Starting revision: `9b63f56d5a4759e25d09ce1ea032f2dd8158dea4`
- Final code revision: `e2ba8a8962e9a9e37526ca626c66ad9532ba160e`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

All required criteria were measured and passed.

### Implemented repairs

`e2ba8a89` adds an allocation-free owned-size calculation for resident stored rank keys. It reads the bound sort column and resident key by reference, uses checked arithmetic for the destination `RankKey`, parts vector, collection and record-ID storage, and variable string/ID bytes, then admits the cursor before constructing its key. Both lifecycle cursor insertion and exact-page cursor insertion use the same prediction. The materialized record ID now uses exact-size `String::from` storage so the prediction matches the constructed destination.

The same commit selects the general executor before its first restriction application when the request is already known to be a multi-file residual page: more than 256 active rows, more than 64 files, multiple workers, a page bound at most 64, and an unindexed/source-dependent filter. This removes the ordered and exact-page probes that would otherwise compute the same bounds before falling back. The separate exact-OR membership recount was removed; `exact_posting_matches` is now derived from the `must` membership already produced by the selected executor.

The one-file and 65-file fixtures now contain the same 260 intrinsic-ID records, four logical rows per file in the multi-file layout, with the specified residual exceptions. The benchmark uses the same generator and segment configuration.

### Cursor allocation observations

The actual long stored sort keys remained `20,968 B` each with `384 B` of fixed heap/reader state. Limits were derived from these observed values.

| Run | Reviewed-tip constructions | Repaired constructions | Result |
| --- | ---: | ---: | --- |
| Initial key alone exceeds limit | 1 for slot 0 | 0 for slot 0 | Typed resource error, no callback, no partial cache |
| Initial fits; successor exceeds overlap | 2 for slots 0,1 | 1 for slot 0 | Typed resource error at sequence 0, no callback |
| Sufficient cold cache | 2 | 2 | Reference rows/count; peak covers both live keys |

The test-only observation is at the first instruction of the owned `segment_rank_key` construction, before its parts-vector reservation or stored-key clone. It records the segment and slot. The 42,319-byte overlap boundary remains only a fixture value, not production policy. The sizing test compares the shared checked formula against materialized keys with empty, numeric and 16 KiB string parts.

### Residual restriction observations

The corrected 65-file fixture was instrumented below recursive Boolean composition. Every observation records field, operator, literal, segment ID, and the complete effective local input slots.

| Entry point | Starting `a` posts | Starting `note` posts | Repaired `a` posts | Repaired `note` posts |
| --- | ---: | ---: | ---: | ---: |
| Public stream | 3 | 2 | 1 | 1 |
| Ordinary search | 5 | 4 | 1 | 1 |

Each repaired post used segment `1` and input slots `[0..259]`; there was no genuinely narrower input. Both residual entry points use the general fallback (`ordered_stream_completions == 0`). The ordinary call dispatched 65 parallel source-file jobs for 65 distinct files, opened more than 64 files, scanned source records, and parsed source data. This preserves file-parallel residual verification.

The exact indexed OR still uses the ordered route for both entry points, applies `a` and `b` exactly once each, and performs zero file opens, source-byte reads or parses. The one-file and 65-file layouts return the same stable IDs, order, fields and count.

### Corrected fixture and residual correctness

- Exact indexed OR: both layouts return count `260`, a 50-row page at offset 1, and full reference-parity fields. The 65-file page has zero source work.
- Residual `a = true || note = 'needle'`: both layouts contain 260 records and the query returns `257`. The untimed full offset-zero result equals the reference and contains none of IDs `1`, `5`, or `9`.
- The nested Boolean evidence regression remains green: cold stream A has `["a","c"]` with count 3, and the warm cache page returns B/C with their reference fields.

### Corrected release comparison

The corrected benchmark was built identically against `9b63f56d57` and `e2ba8a8962`. Each layout used four workers, a cold disabled verified cache, count on, offset 1 and limit 50. Times are median first nonempty callback / completion over three alternating baseline/candidate pairs.

| Layout | Baseline µs | Candidate µs | Rows / count | Source counters baseline → candidate |
| --- | ---: | ---: | ---: | --- |
| One file | 42.327 / 48.462 | 43.560 / 49.800 | 50 / 260 | 0 files, 0 bytes, 0 parses in both |
| 65 files | 64.555 / 72.728 | 43.345 / 48.939 | 50 / 260 | 0 files, 0 bytes, 0 parses in both |

The one-file difference is within host variation. Both benchmarks validated retained order and evidence against the reference before the timed stream, and the ordinary result was validated separately after timing.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 633 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 632 passed and 16 ignored under the workspace feature set.
- Focused `rustfmt --edition 2024 --check` passed for every task-owned Rust file.
- `cargo fmt --all -- --check`: only the inherited extra blank lines in `crates/core/src/model/json.rs` remain.
- `git diff --check`: passed.

### Delivery and cleanup

The implementation and benchmark commits are pushed on the integration branch. The detached baseline worktree and benchmark target were removed after all measurements. Task memory was removed after its findings were transferred here. The original checkout's untracked task source was removed only after the committed 65-line specification was verified byte-for-byte. The integration branch and worktree remain for owner review and merge. No criterion remains unverified.
