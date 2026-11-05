# Library extension review: query planning and selected-source lifecycle

## Revisions and scope

Starting checkout: `4cea642f82904b83a3676eb87731a198c746788e` (clean before measurements). Delivered production revision: `bb7d812f973a27f611cef193f56efb2d34657f03`. The reviewable sequence is Stage 1 `35dbf96548`, Stage 2 `5fa07ccf49`, Stage 3 `31cf753ac2`, then the bounded cost-estimate refinement `bb7d812f97`. Documentation and benchmark-report commits after this revision do not change production code. The reference and final candidate release binaries are preserved locally in ignored `reference-bin/` and `candidate-bin/`; SHA-256 values are `fa5bfe1390ca6d404da66c67871d8b05345ce1fd1e94eb446ab611ce8b096c0e` and `99cd5acb079959f1e065a317e195a34b1bd8eec91eb91f9a253651a2a11fab83`.

[Stage 1](STAGE1.md) repairs logical JSON node traversal, refresh ordering, derived policy parity, malformed native JSON handling and cache publication. [Stage 2](STAGE2.md) adds configured logical-field typed/presence postings, native-to-selected candidate translation and compatible multi-field text candidates. [Stage 3](STAGE3.md) keeps generation-bound source order, enumerates sparse candidates directly, remaps unchanged typed rows, verifies changed bigram-overlay records and delivers unsorted selected streams during scanning. The final refinement uses cheap B-tree cardinality estimates to avoid constructing a broad scalar bitset when a selective text condition is already available.

## Query and lifecycle map

A `JsonFilter` or `TextQuery` is compiled and bound once per collection. Its typed plan and text representation/operator binding identify safe necessary conditions. A selected snapshot owns sorted source descriptors, a default-order rank and the generation fingerprint. A native picker posting covers only its filesystem JSON slots; a generation-bound map translates those slots, and custom slots remain in the fallback domain. A selected typed posting covers configured logical fields across all selected slots. Compatible representation bigrams cover their bound field semantics. The planner uses a bounded cardinality/cost heuristic: a rare scalar or text condition can seed verification; a broad redundant posting need not be materialized. AND retains safe conditions, unsupported OR branches broaden coverage, and approximate NOT is never complemented. Exact predicates retain their evaluation order and errors. Sparse candidate slots are visited directly, while a completed ordered-cache hit skips candidate planning, slices the requested range and reads only its page.

Refresh reconciles native inventory first, then selected inventory. With a small delta, retained typed rows are remapped and only changed records are decoded for active typed outputs. Immutable text postings keep a base-to-current map and changed-slot overlay. Deletions are excluded; additions and edits are verified. A delta or accumulated overlay over one quarter of the selected collection, missing old postings, or changed semantics triggers a full selected rebuild. Provider discovery can still enumerate the whole collection, and remapping old typed rows costs in-memory work proportional to those rows. The selected inventory, postings and cached ordered slots belong to one generation. A failed build keeps the prior view unavailable for retry rather than publishing partial state.

Selected manifest version 2 and text companion manifest version 5 reject older metadata. Version 5 records base source signatures, slot mapping, overlay, semantic identity and filter digest; a deliberately corrupted but syntactically valid map is rejected and rebuilt in the downstream test. Derived snapshots retain `OBJDRV02`; native JSON snapshot bytes are unchanged.

## Focused correctness and work evidence

`review_stage1.test.rs` compares native JSON and built-in-decoder custom JSON through complete logical structure, including root arrays, empty containers, escaped/numeric keys, paths and derived extraction. It exercises all freshness modes, count caps, malformed native JSON, rebuild after edits/additions, and the native/selected refresh order. `review_stage2.test.rs` compares full ordered IDs with a direct cache-disabled oracle across a mixed collection and another native collection; a rare native term reads fewer than 20 sources versus at least 193 direct reads. It covers unsupported OR fallback, a three-record deep cached page with zero candidate visits and exactly three descriptor visits/reads, opposite scalar/text seeds, two text fields, and restored ordinary custom scalar postings. For the rare-text fixture, zero broad scalar bitsets are materialized; the rare-scalar fixture uses one. Existing fallible-operator and Boolean short-circuit tests still pass.

`review_stage3.test.rs` warms text, configured scalar and derived postings over 64 custom records. One edit performs one source read and one parse during refresh; delete plus rename reads at most one new source. Ordinary indexed queries and a fresh direct oracle agree after save/reopen, and the overlay restores with zero source reads. It rejects a tampered base-slot map. Unsorted selected JSON/text callback cancellation stops after at most two reads and does not publish a completed ordered entry. Returned records still receive a digest; nonreturned candidates do not. Sorted selected streams prepare their bounded sort before callback delivery.

`cargo test --locked --workspace --all-targets -j 2` and `cargo clippy --locked --workspace --all-targets -j 2 -- -D warnings` passed on the delivered production source. Changed Rust files were run through rustfmt and the repository spacing procedure. The whole-repository `python3 scripts/format-rust-spacing.py --check` remains failing: the [captured output](spacing-check.log) includes pre-existing formatting differences in `bigram/filter.rs`, `config/discovery.rs`, `extensions.test.rs` and `tests/index_build_profile.rs`, plus spacing/rustfmt disagreement at two selected-text lines and one downstream assertion. No unrelated whole-workspace reformat was applied.

## Native 152-case regression comparison

The [baseline manifest](baseline-manifest.json) records Rust/Cargo 1.99.1, Linux host, 16 Rayon threads pinned to CPUs 0–15, configured Musenalm schemas, the 148,486-record corpus fingerprint and 152-case workload checksum. The same files, release options, ordinary ASCII encoding and cache preparation were used on both revisions. Exact commands, timestamps, exit codes and stderr are in [reference commands](reference/commands.json) and [final candidate commands](candidate-final/commands.json). The invocations were:

```sh
cargo build --locked --release -p objekte-bench --bin json-query-mix -j 2
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/query-planning/reference-bin \
  --output-dir crates/bench/results/library-extension-candidate/query-planning/reference \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
env RAYON_NUM_THREADS=16 taskset -c 0-15 python3 crates/bench/run_library_query_matrix.py \
  --bin-dir crates/bench/results/library-extension-candidate/query-planning/candidate-bin \
  --output-dir crates/bench/results/library-extension-candidate/query-planning/candidate-final \
  --runs 7 --only json-schema-ascii-cache0,json-schema-ascii-cache64
```

The [reference](reference/signatures.json) and [candidate](candidate-final/signatures.json) 152-case signatures match at both budgets. For every case, recorded source reads, parses, candidate scans, predicate-cache hits and ordered-cache hits are identical. [Per-case results](comparison-final.json) contain medians, min/max, absolute and relative changes, signatures and source work; raw seven-round samples are in [reference](reference/) and [candidate-final](candidate-final/).

| Query cache | Sum of per-case reference medians | Sum of candidate medians | Difference |
| --- | ---: | ---: | ---: |
| 0 MiB | 10,859.96 ms | 10,999.24 ms | +139.28 ms (+1.28%) |
| 64 MiB | 9,810.36 ms | 10,034.06 ms | +223.70 ms (+2.28%) |

These are sums of case medians, not a single end-to-end workload timing or a stable tail-latency estimate. The largest first-pass differences were status metadata and broad sorted queries. Three alternating reference/candidate pairs of the 12 suspect cases were run at 64 MiB with seven rounds each. [Commands/raw output](suspect-repeat-final/) and [paired summary](suspect-repeat-final/summary.json) are retained; all signatures and source-work counters matched. Median paired differences ranged from +3.4 to +10.4 ms, or +0.7% to +3.1% per case. The largest were `@modified ASC && @id DESC` (+10.4 ms, +1.9%), broad annotation descending (+10.0 ms, +1.3%), broad size (+9.3 ms, +1.7%) and modified filter/sort (+8.5 ms, +1.6%). Earlier 0/64 MiB status-only repeats on Stage 3 revision `31cf753ac2` are saved separately in [status-repeat](status-repeat/); they are preliminary evidence and are not labeled as final-revision measurements.

The repeated subset confirms a small timing increase despite equal recorded source work. No source/parse/cache regression or large per-query gap was observed. The exact cause of this small native timing difference was not isolated; these selected-source changes do not send JSON-only scopes through selected record evaluation. The normal benchmark cannot measure every custom-format cost. Its focused tests instead assert candidate visits, descriptor visits, decodes and refresh reads. The selected exact evaluator still owns a logical tree for each verified custom candidate; optimizing that representation remains separate work. No allocation-instrumented or broad corpus campaign was run under this handoff.
