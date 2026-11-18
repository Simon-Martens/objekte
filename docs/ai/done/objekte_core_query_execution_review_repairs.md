# Objekte core query execution review repairs

**Target branch** `integration/query-execution-refactoring`
**Reviewed head** `0738c7a959bbdac255e37c90a111bf70778697a5`
**Reviewed implementation** `4b8fb2207b`
**Comparison revision before the completion work** `4dd349ade31eae4edc2838fc7860c91f6368f2e7`

Continue the existing task in its integration worktree. Scope production changes, tests and benchmark code to `crates/core`. Preserve the public query, result and stream APIs, on-disk formats, query semantics, ordering, cancellation and source-error behavior. The owner merges the integration branch. This document supplements the existing completion specification and supplies concrete failures still present at the reviewed head.

## What already works

Retain `native_segment_selects_posting_or_text_first_for_4096_records`. In its selective FILTER case the eight numeric posting matches become five FTS candidates and three verified records, IDs `0`, `1536` and `3072`. Its converse case first uses FTS to reduce 4,096 records to two. The ASCII and incremental Unicode result-digest files in the final report are byte-identical to their corresponding merged-baseline artifacts. Q030 and Q141 now give the expected totals on those runs. The work below must preserve these results. [Native fixture](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/run.test.rs#L712) · [Completion report](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/docs/ai/generated/query-execution-completion/REPORT.md)

## R1. Restrict native range postings to the effective logical input

**Priority 1.** The direct posting adapter passes the complete logical domain as the coverage argument and the mixed effective input as the restriction argument. The scalar range kernel skips its input mask whenever `input.len() >= domain.len()`. That shortcut relies on the input already being a subset of the logical domain. Raw-fallback slots invalidate that assumption. An exact numeric range can consequently reintroduce a logical record removed by a preceding restriction or a deletion. Constructing the returned membership can also trip the subset assertion in debug builds. This is a source-level defect. Reproduce it on the reviewed revision before changing the code. [Caller](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_access/segment.rs#L304) · [Range kernel](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/posting.rs#L505)

1. Construct a native segment with logical slots `{0,1}` and raw-fallback slot `{2}`. Give slots 0 and 1 numeric values 10 and 20. Use an indexed numeric field without a sort column. Restrict the operation to `{0,2}` and evaluate `value >= 20`. The old kernel can return slot 1. The required numeric proof is empty. The adapter may retain slot 2 for residual handling according to its fallback contract. Slot 1 must be absent from both outputs.
2. Make the provider distinguish the pinned coverage domain, the current effective input, and its logical intersection. Retain the full logical domain for posting coverage validation. Pass only the effective logical input to lookup kernels that assume this containment. Reuse the effective handle when the containment is already known. An unrestricted kernel needs positive knowledge that its domain is complete. Cardinality alone cannot establish set equality across these two domains. Review scalar, observed-scalar and temporal range helpers that share this convention.
3. Build `possible` and `proven` by ownership transfer. Reuse or move the result handle before adding fallback candidates. Keep the invariants `proven ⊆ possible ⊆ effective input` for every proposal, including a partially covered exact posting. Never promote fallback slots to proven through that posting.
4. Exercise the adapter directly for the exact sets above. Add a public pinned-view regression with a mixed published segment and an inactive matching logical record, or an equivalent earlier predicate removing that record while retaining a raw-fallback record. Confirm slot placement and that the direct range posting was selected. Compare search and exact count, with evidence disabled and enabled, to the independent reference. Also run the focused regression in release mode because the subset assertion is debug-only. A result that widens and gets intersected away later fails the adapter test.

## R2. Make execution diagnostics describe the work that actually ran

**Priority 1.** The committed ASCII artifact for Q181 searches `entries` for `Krause` through `agents[0].agent.name`. Both the ordinary run and the diagnostic run count 4,928 source candidates and six matches. The same diagnostic object marks `plans[0].candidates = 0` as exact. Zero belongs to a relation-target observation. It was already incorrect at the merged baseline. Q141 exposes a second problem. All six committed observations are tagged `ordered_page`, including the follow-up pass. [Raw plans](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/docs/ai/generated/query-execution-completion/final-4b8fb220/ascii-off/query-plans.jsonl) · [Current aggregation](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L3196)

1. At the points that increment ordinary source candidates, record that same contribution by collection, pinned domain and execution phase. Include scans without a restricting index and relation-source fallback. If ordinary execution replaces page counters with exact-count counters, select the matching set of contributions at that same result boundary. Keep physical leaf inputs/outputs, target work, combined source candidates, residual evaluations and final matches distinct. Remove the global phase search and last-node-per-domain reconstruction. Attribute actual opens, reads, parses and relation work to the collection where the corresponding ordinary counter is charged. Where an actual is unavailable, preserve the existing API's unknown/estimated representation.
2. Use one optional request-owned recorder. Pass an explicit scoped phase to nested executions and worker jobs. Close scopes on success, error and early return. Migrate fused, skipped, relation and ordinary observations, then remove thread-local capture fallback and the process-wide capture log. Preserve prepared occurrence IDs for repeated predicates. Separate the decision to record from limits and cancellation. Currently ANALYZE constructs an execution context, while `context.is_none()` controls exact ordered paging and parallel scan eligibility. For identical controls, ordinary and analyzed execution must choose the same strategy. Collect observations without formatting or allocating trace data on an unobserved search. [Dispatch](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L2085) · [Request capture](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/request.rs#L63)
3. Assert these source totals in the committed corpus configuration. Q030 gives 2,251 candidates / 1,264 matches, Q141 gives 982 / 823, and Q181 gives 4,928 / 6. Preserve Q181's independent relation-target work. Add a small native fixture with two disjoint one-record OR branches so the combined count is two. Exercise two collections, multiple domains, a page followed by exact count, a verified-cache hit and concurrent requests. Use a test-only strategy recorder or counters on the real kernels to compare ordinary and analyzed ordered/parallel paths. For a warm replay, report actual replay work and identify any separately obtained cold diagnostic run in the benchmark evidence.

## R3. Select executable indexes through one interface for all query shapes

The native provider suppresses its posting offer as soon as a sort column exists on the field. Its general fallback calls `segment_expr_bounds` to select a kernel during execution. The main loop estimates FILTER only for one leaf. Ordered streaming applies FTS to all active slots even after FILTER has narrowed the same segment. The committed Q141 page pass demonstrates this directly. Its two year predicates reach 3,308 survivors, then FTS starts again at 4,928. [Offers](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_access/segment.rs#L125) · [Single-leaf estimate](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L2135) · [Ordered traversal](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L3610)

- In `query_access`, register one prepared operation per applicable physical alternative. Begin with simultaneous numeric posting and sorted-column offers. Cover the existing metadata, position, gram, relation and residual routes as they are migrated. Carry the actual operation identity, domain, prepared predicate occurrence, coverage, proof strength and work/output estimates. Application executes the selected handle on the supplied input. Give residual verification its own operation. Keep storage layout, native indexes and matcher-specific branching private to their owners.
- In `query_plan`, enumerate legally reorderable positive conjunctions including FTS. Compare lookup cost plus the cost of subsequent work on estimated survivors, including verification where required. Clamp estimates to current effective cardinality. Use stable tie-breaking and cheap existing statistics. Preserve explicit Boolean boundaries, short-circuit/error behavior, unknown values, and all requested evidence. OR combines alternatives within the same incoming domain. NOT complements within that domain. Use the same selection path from segmented search, exact count, ordered paging and ordered streaming. A legitimate exact-count pass may re-evaluate work. Label and measure each pass separately.
- Extend the 4,096-record fixture with a field having both indexes and a compound FILTER whose selective leaf appears second. Assert offers, chosen operation, actual input/output and verified IDs. Exercise both existing FTS/FILTER distributions through ordinary, count, sorted-page and ordered-stream paths. Distinguish traversal order from predicate evaluation order. On each reducing conjunction the second operation must receive the first operation's survivors. For the ordered case, force the ordered path and assert that it ran. Also run an independent test adapter with a different backing through the public query path. It must be selectable using the same offer and membership contract.

## R4. Preserve ownership through the next consumer and account for live data

In `search_segmented_impl`, `bounds.may.as_ref().clone()` copies the FILTER result immediately before FTS receives it. Cloning the inner bitmap copies membership even when an earlier stage correctly shared its `Arc`. Ordered paging also clones each emitted hit. Repository and lifecycle stream wrappers copy a complete batch when a row needs translation. Cached streaming scans every prior row twice when appending a row. [Bitmap handoff](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L2159) · [Page collector](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_execution/runner.rs#L1763) · [Cache replay](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_results/cache.rs#L388)

- Carry effective membership as a borrowed, shared or moved handle through bounds, Boolean evaluation, proofs and provider calls. Bind snapshot borrows to the pinned request. Forward identity results, including narrowed inputs, with their original owner. Recognize proven-all without materializing a second set. New set algebra may allocate its output once. Use in-place algebra where ownership is unique and doing so preserves all live proofs. Stop materializing an active bitmap anew at each unchanged boundary.
- Separate internal row ownership from the public borrowed callback. The internal page collector accepts owned rows or batches. Finalize revision and diagnostic fields while the producer owns each row, then lend the external callback a slice. Preserve payloads and evidence through provisional replacements, cancellation and sink errors. Remove the cloning collector and both ordered/replacement batch remaps in `repository.rs` and `lifecycle.rs`.
- In cache replay maintain a running page-byte total and compute the byte size of each appended row once for retained accounting. Keep event limits separate from total buffered bytes. In `query_results/delivery.rs`, account for both large keys and materialized payloads at their simultaneous peak. The current subtraction removes the page winners' key bytes while those keys remain live in `owners`. Count each retained allocation once, including live identity and scratch storage under the existing budget contract. [Delivery](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/crates/core/src/query_results/delivery.rs#L73)
- Instrument the producer, proof and next provider inside the real public request. Assert the same backing for identity handoffs and zero deep copies at those boundaries. Include borrowed/moved/shared inputs and mixed logical/fallback cases. Track the same large row payload across two replacements and a mixed diagnostic-remap batch. Derive the expected live requirement independently of the counter under test. Test large variable-size sort keys plus a large materialized payload, a budget exactly at the requirement, a budget one byte below it and an independently exceeded event limit. For cached replay, double the number of rows and assert linear retained-size visits using a counter.

## R5. Give the remaining execution code a production owner

The reviewed runner grew from 6,903 to 6,926 lines. The executor fell from 3,980 to 3,838. Moving the provider was useful, but ordered cursors, ranking and much of physical execution still require edits to the coordinator. Make the following moves in compiling commits. Use existing modules first.

| Destination under `crates/core/src` | Production responsibility |
| --- | --- |
| `query_results/ordered.rs`, with private children if needed | Move exact segment ordered-page traversal, lifecycle ordered page/stream traversal, cursor maintenance and ordered record fetching from `query_execution/runner.rs`. |
| `query_results/{ranking,materialize,delivery,cache}.rs` | Move `offer`, `offer_merged` and winner accounting to ranking. Keep materialization, owned-row collection, cache replay and callback delivery under their respective owners. |
| `query_access/segment.rs` and private segment submodules | Move the remaining native lookup implementations out of `executor.rs`. Keep native storage access and the prepared operation table together. Replace fallback kernel selection with the explicit offers from R3. |
| `query_plan/{boolean,selection,ordering,operations}.rs` | Own legal term order, survivor estimates, physical operation selection, traversal and early stop. Wire policy into real callers and remove superseded helpers. |
| `prepared.rs`, `query_predicate/` | Assign each predicate occurrence its ID when binding. Carry that ID through Boolean planning, application, verification and evidence. Put exact matching and reader capability selection here. Preserve the independent reference evaluator. |
| `query_execution/{request,domain,segment_scan,source_scan,relations}.rs`, `query_diagnostics/trace.rs` | Execute prepared domain jobs, supply bytes, collect actuals, then render them. Remove the `json_decoded_scalars` reader-name check from the scan loop. Bind the fast reader capability once with a conservative fallback for other readers. |
| `run.rs`, `query_execution/runner.rs` | Keep entry points and orchestration. Split `search_segmented_impl` and `search_impl_with_collector` by the responsibilities above. |

Record the resulting owner and actual production caller for each migrated responsibility. Prove replaceability with a test-only exact matcher registered at the prepared-leaf boundary. Through the public base and segmented engines, exercise unproven rows, AND, OR, evidence and unavailable input. Count calls inside the replacement matcher. Replacing a storage adapter or matcher must require edits only in its implementation and registration. Moving a function and leaving its physical decision in the coordinator does not satisfy this criterion.

## Performance investigation and delivery

Q164 is the field-scoped regex `Almanach|.*` with evidence and exact count. It scans 4,928 records. The earlier focused ASCII medians were **6.123 and 6.434 ms**. The reviewed implementation recorded **7.577 ms** across 20 samples and **7.017 ms** across 40 samples, with the same candidate, parse and byte counts. These runs were collected separately. They establish an open performance question, not its cause. [Earlier focused run](https://github.com/Simon-Martens/objekte/blob/4dd349ade31eae4edc2838fc7860c91f6368f2e7/docs/ai/generated/core-query-execution-corrections/q164-final-4baa-second/query-results.csv) · [Reviewed focused run](https://github.com/Simon-Martens/objekte/blob/0738c7a959bbdac255e37c90a111bf70778697a5/docs/ai/generated/query-execution-completion/final-4b8fb220/q164-focused-2/query-results.csv)

1. Land R1 and its regressions first. Add the Q181, Boolean and phase assertions alongside R2. Implement R3 and bitmap ownership together where they share an API. Separate row/accounting changes and mechanical module moves in the commit history. Keep the existing worktree and branch workflow.
2. Compare release binaries for `4dd349ad` and the repair head on the same Musenalm corpus, schema, machine, four workers and disabled result cache. Alternate baseline and repair blocks for Q164, then reverse their order. Keep raw samples, per-block medians, candidate/parse/byte counters and worker/scan strategy. Time ordinary execution only. If the difference repeatedly exceeds **both 5% and 0.2 ms**, profile the ordinary build and isolate the responsible change, using `0738c7a9` as an intermediate comparison if needed. Keep the performance item open if it remains unexplained. Preserve regex, field-selection and evidence semantics.
3. Run the following checks and the existing short acceptance matrix once on the integrated repair. ASCII cases are Q001, Q007, Q030, Q035, Q084, Q127, Q141, Q150, Q163, Q164 and Q181. Incremental Unicode cases are Q030, Q141 and Q163 with cold and verified-cache-warm profiles. Use five repetitions for the matrix. Reserve longer reruns for flagged cases.

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

4. Under `docs/ai/generated/query-execution-completion/`, publish a repair report with base and final SHAs, test names and outcomes, raw Q181/Q141 traces beside ordinary counters, restricted-posting sets, allocation identities, live-byte accounting, symbol ownership and Q164 paired timings. Compare ordinary results and digests to the merged baseline. Include any unverified item explicitly. Present the integration branch for owner review once the evidence meets the criteria above.

## Implementation report

- Task: `query-execution-review-repairs`
- Completion date: 18 November 2026
- Reviewed base: `0738c7a959bbdac255e37c90a111bf70778697a5`
- Final implementation: `1afdf73852`
- Final integration tip before document lifecycle: `ba4528764a`
- Integration branch: `integration/query-execution-refactoring`
- Integration worktree:
  `/home/simon/source/objekte-integration-query-execution-refactoring`

### Summary

R1 now masks numeric range postings with the effective logical input. The
mixed logical/raw-fallback adapter regression covers logical slots `{0,1}`,
fallback slot `{2}`, effective input `{0,2}`, and `value >= 20`; the possible
set is `{2}` and proof is empty.

R2 reports the Q181 ordinary source candidate count of 4,928 beside the
relation-target work, rather than replacing it with the target observation's
zero. Q030 remains 2,251 candidates / 1,264 matches and Q141 remains 982
candidates / 823 matches.

R3 retains simultaneous native posting and fallback/sorted-column offers,
effective-input masking, the 4,096-record FILTER/FTS fixture and a compound
FILTER with the selective leaf second.

R4 now shares the FILTER bitmap as an `Arc` through the FTS boundary and uses
copy-on-write only for a genuinely changed set. Cache replay maintains a
running page-byte total instead of rescanning the growing page.

R5 moved ordered cursors and ordered record fetching to
`query_results/ordered.rs` and winner admission/ranking policy to
`query_results/ranking.rs`. Native segmented offers and application remain in
`query_access/segment.rs`.

Q164 is closed by owner decision. Alternating runs show host variance with
identical candidate, parse, byte, scan and result counters. The detailed
timings remain in `REVIEW_REPAIRS.md`.

### Commits

- `338cbbfae7` — segmented range mask and Q181 source-candidate diagnostics.
- `01cfe913ba` — move ordered cursor/fetch and winner policy ownership.
- `1afdf73852` — share the bitmap through FTS; incremental cache page bytes.
- `ba4528764a` — close Q164 and update repair evidence.

### Validation

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo test --release -p objekte-search-core numeric_range_posting_excludes_retired_logical_slots_before_fallback
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

The core suite passed 571 library tests, every integration test and the doc
test. Sixteen existing manual/profiling tests remain ignored.

### Evidence

- `docs/ai/generated/query-execution-completion/REVIEW_REPAIRS.md`
- `docs/ai/generated/query-execution-completion/review-repairs-338cbbfa/`
- `docs/ai/generated/query-execution-completion/review-repairs-q164/`

### Cleanup

No subordinate task worktree remains. The temporary sparse comparison worktree
was removed. The integration worktree and branch are retained for the owner’s
merge.
