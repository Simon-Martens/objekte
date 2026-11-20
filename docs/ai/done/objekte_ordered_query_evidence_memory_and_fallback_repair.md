# Objekte ordered query execution repair specification

**Merge condition.** Hold `integration/query-execution-path-repair` until three regressions pass on the public query APIs. The first can return incorrect matched fields to callers and persist them for later pages. The second can admit a stream operation above its memory budget. The third repeats index work on multi-file searches.

**Starting point.** Reviewed branch tip `6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5`. The last implementation commit was `ca0f3451fa`. Continue in `/home/simon/source/objekte-integration-query-execution-path-repair`. Keep the primary checkout on `master`. Follow `docs/ai/README.md`, push each integration commit, and leave the merge to the owner.

## 1. Preserve row-specific matched fields for nested Boolean filters

The ordered runner builds per-branch evidence when the root is an OR. For an AND containing a heterogeneous OR, the index can prove membership exactly while the set of matched fields still varies by row. The runner skips verification, finds neither static fields nor a branch result, and can return and cache an empty list. [Ordered admission](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L3532-L3586) · [Evidence and cache insertion](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L3994-L4034) · [Reference Boolean semantics](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/reference/filter.rs#L145-L173)

**Repair.** For each emitted row and each admitted cache member that needs evidence, supply fields proven for that row. Distinguish known empty evidence, such as a successful negation, from unknown evidence. The smallest acceptable change is to decline an unsupported expression before the first callback or cache publication and use the existing evaluator that supplies complete evidence. Preserve the zero-source path for supported top-level indexed OR queries and the existing metadata comparison rules.

**Acceptance.** Extend the existing ordered-OR fixture. Use one segment, indexed booleans, stored sort order A through E, locator output, evidence and count enabled, and an initially empty verified-result cache large enough for the complete entry. Query `c = true && (a = true || b = true)`. [Existing public-API fixture](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/tests.rs#L6865-L7000)

| ID | True fields | Expected result |
| --- | --- | --- |
| A | a, c | `["a", "c"]` |
| B | b, c | `["b", "c"]` |
| C | a, b, c | `["a", "b", "c"]` |
| D | a | Excluded |
| E | c | Excluded |

Run a cold `IndexView::search_stream` with limit 1 through completion. Require A and count 3. On the same pinned view, call `search` with offset 1 and limit 2. Require an actual verified-result cache hit, B and C in order, and count 3. Check literal fields and full-hit parity with `search_reference`. Run a cold ordinary search with caching disabled as a separate control.

## 2. Account for the popped sort key through its last use

The heap subtracts the cursor charge at pop, then queues the successor. The popped cursor still owns its sort key across a possible source read and passes it to the cache builder. Two long keys can therefore coexist while the budget covers only one. [Cursor replacement](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L3813-L3834) · [Bounded source read](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L3907-L3935) · [Later use of the key](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L4019-L4034)

**Repair.** Track the current key’s owned bytes until release, or change the order of operations to end its lifetime before constructing the successor. Keep the reserved heap storage, heap keys, current key, reader, page, source bytes and delivery identities distinct. Apply the existing byte-estimation convention once per live allocation at successor admission, source read and delivery. Carry the same remainder into the bounded reader. Preserve the independent cache budget and reuse the existing rank key. The per-row accounting must remain constant time. [Key sizing](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_results/ranking.rs#L752-L765)

**Acceptance.** Use consecutive long stored **string sort values**, one segment, an accepting cache, and a residual predicate that reads source data. Observe key sizes from the actual keys, independently of the budget counter under test. Choose a legal stream limit that admits either key alone but falls below their measured overlap; keep individual record and event limits valid. If the keys overlap, require a typed resource error with correct progress before the operation that would exceed budget, no later callback, and no partial cache entry. If the repair eliminates the overlap, demonstrate the two lifetimes and test the resulting live-byte boundary. With a sufficient budget, require reference-equivalent rows and count. Retain the existing replacement-callback and counted-tail regressions.

## 3. Apply each index restriction once across multi-file fallback

The ordered probe computes segment memberships before counting distinct candidate files. With more than 256 candidates, multiple workers, and more than 64 candidate files, it declines. The general executor receives the prepared query and a flag indicating the prior attempt, then derives bounds again. The previous indexed-OR benchmark put all 1,000 rows in one JSONL file. [File-count decision](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L3686-L3719) · [Fallback call](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L4538-L4550) · [General bounds](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/src/query_execution/runner/mod.rs#L2182-L2204) · [Benchmark layout](https://github.com/Simon-Martens/objekte/blob/6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5/crates/core/examples/bench_query_execution_addendum.rs#L389-L398)

**Repair.** Select the route earlier if equivalent eligibility is available cheaply, or move the already computed restrictions into the selected executor. An exact indexed query may finish on the ordered route if measurement supports that choice. Preserve file-parallel verification for residual queries. Reused bounds must match the pinned revision, collection, segment, predicate and effective input slots. Preserve possible and certain membership and any row-specific evidence. Use query-local ownership and avoid bitmap copies or a new cache.

**Acceptance.** Use four workers and 65 JSONL files with four matching rows each. Mix a-only, b-only and a+b records, declare both booleans indexed, and sort by a unique stored value. Run the public stream and ordinary search cold with caching disabled, locator output, evidence, count and a nonzero offset. Record the actual route and count physical posting applications across the whole invocation, below recursive filter composition. Require at most one application per identical term, segment and effective input set. For each entry point, check ordered IDs, fields and count 260 against the reference. Record file opens, source bytes and parses. Repeat with the same rows in one file and retain zero source reads for the indexed locator control. Exercise a residual multi-file query to confirm file-parallel execution remains available.

## Verification and handoff

Capture the starting and repaired observations for each acceptance case. Run three alternating release baseline/candidate pairs for the one-file and 65-file indexed OR fixtures, recording worker count, cache state, first nonempty stream callback and completion time. The two small fixtures cover the routing boundary directly.

Run `cargo test --locked -p objekte-search-core --all-features`, `cargo test --locked --workspace`, `cargo fmt --all -- --check` and `git diff --check`. Record the inherited `crates/core/src/model/json.rs` formatting difference separately if it persists. Append the observations, commands, and final code revision to the completed spec under `docs/ai/done/`. Report each merge condition as passed, failed or unverified and give the pushed report revision.

## Implementation report

- Task identifier: `ordered-query-evidence-memory-and-fallback-repair`
- Completed: 20 November 2026
- Starting revision: `6e79b10957962c5f3fe4a32bcd1fbbcb4a99e1c5`
- Final code revision: `f7b536f5eec3d7194660c22d77944742e85b392b`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

All three merge conditions passed.

### Implemented repairs

- `ae4f00eff6` — ordered execution now declines a filter whose evidence is row-dependent unless it is the supported top-level exact OR form. The existing evaluator consequently owns an AND containing a heterogeneous OR and supplies complete row fields before either streaming or cache publication.
- `79098483a4` — the popped cursor's rank key remains charged through successor admission, source verification/read, page and delivery checks, and cache admission. Upcoming source bytes and verification bytes retained for locator delivery are included separately; bounded readers receive the same remaining allowance.
- `8d4b31d55d` — exactness is calculated before the multi-file fallback decision. An exact indexed query with more than 64 candidate files remains on the ordered executor instead of recomputing restrictions in the general executor. Residual queries still decline to the file-parallel path.
- `3a9a07581f` — adds release cases for the one-file and 65-file indexed OR routing boundary.
- `f7b536f5ee` — asserts that the successful live-key run reports a peak at least as large as the independently measured key overlap.

### Merge condition observations

**1. Nested Boolean evidence: passed.** On the starting revision the cold `IndexView::search_stream` for `c = true && (a = true || b = true)` returned A with `[]` and then cached that incomplete evidence. The repaired cold stream returns A=`["a", "c"]` and count 3. A subsequent offset-1/limit-2 ordinary page is an actual verified-cache hit and returns B=`["b", "c"]` then C=`["a", "b", "c"]`, count 3. D and E are excluded. The cold cache-disabled ordinary control equals `search_reference`.

**2. Live popped-key accounting: passed.** The string-sort fixture independently observed base `384 B`, first key `20,968 B`, and successor key `20,968 B`. The legal test limit is `42,319 B`: base plus either key fits, while both keys overlap above it, and the event limit remains 8,192 B. The starting revision completed the operation at that limit. The repaired stream returns a typed resource error at sequence 0 and emitted-row progress 0, invokes no callback, and leaves no partial verified result. With 256 KiB, the reference-equivalent rows/count complete and the reported peak includes the measured 42,320-byte key overlap. Existing replacement-callback and counted-tail regressions still pass.

**3. Multi-file restriction reuse: passed.** On the starting 65-file fixture, the public stream used the general `bounded_top_k` path, applied six physical postings, opened 65 files, read 12,175 bytes and parsed 260 records. The repaired stream and ordinary search each complete on the ordered route with exactly two physical applications, zero file opens, zero source bytes and zero parses; both return count 260 and the reference page with evidence. The one-file control also uses two applications and zero source work. A residual 65-file query still opens more than 64 files and scans the multi-file source through the file-parallel path.

### Release comparison

The final benchmark example was compiled against `6e79b10957` and `f7b536f5ee`. Three alternating pairs used four workers and a cold, disabled verified-result cache. Times are the median first nonempty `OrderedRows` callback and completion.

| Fixture | Baseline first / completion | Candidate first / completion |
| --- | ---: | ---: |
| Indexed OR, one file | 39.142 / 42.207 µs | 13.825 / 16.988 µs |
| Indexed OR, 65 files | 344.903 / 352.506 µs | 44.404 / 50.392 µs |

Both fixtures returned 50 rows at offset 1 in the 65-file case configuration, or all three available rows in the four-row control. Counts were 260 and 4 respectively and matched the reference evaluator. The 65-file candidate counters were 51 candidates, 260 exact posting matches, zero files/bytes/parses, while the baseline opened all 65 files and read/parsed the source rows.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 632 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 631 passed and 16 ignored under the workspace feature set.
- Focused `rustfmt --edition 2024 --check` passed for every task-owned Rust file.
- `cargo fmt --all -- --check`: only the inherited extra blank lines in `crates/core/src/model/json.rs` remain.
- `git diff --check`: passed.

### Delivery and cleanup

All implementation and benchmark commits are pushed on the integration branch. The temporary detached baseline worktree and release target were removed after measurement. Task memory was removed after transferring its findings here. The original checkout's untracked task source was removed after the committed 45-line specification was verified byte-for-byte. The integration branch and worktree remain for owner review and merge. No merge condition remains failed or unverified.
