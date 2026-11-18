# Objekte query execution repair specification

**Local implementation assignment · 18 November 2026**

**Scope:** `crates/core` only. Implement and validate the query library through its public Rust APIs, its own tests and its existing benchmark examples. The only deliverables outside that crate are the task record and benchmark evidence under `docs/ai/`. Keep the existing storage formats and public result types. The alternative provider used for conformance is a test-only Rust implementation over sorted slot-ID lists.

Continue the existing branch `integration/query-execution-refactoring`. The reviewed tip is `20e17c5be0d1f6c227503d5681c2bf7c76ca2808`. The original comparison base is `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. This is a corrective pass for the original query execution assignment. The previous completion report describes the starting point. Use the acceptance criteria below to establish completion.

**Deliver one integration branch on which the real query engine can replace a filter algorithm or its index backing independently, combine the resulting slot sets correctly, and return or inspect results without duplicating their buffers.** Preserve the working shared-cache handoff, compact source-recovery locator, public APIs and existing optimized traversals.

## 1. Work order

| Commit group | Deliverable | Evidence before moving on |
|---|---|---|
| 1. Membership and operation identity | Fix empty sets, domain identity, narrowed inputs and stable handles. | The counterexamples in R1 and R2 fail on the reviewed code and pass after repair. |
| 2. Production access path | Route a typed FILTER and a text predicate through the request’s actual provider catalog on base and segmented searches. | Gate A below passes through `Repository::search` with both operation orders and both index backings. A counting provider proves the selected operations ran. |
| 3. Predicate isolation and planning | Complete the other access families, prepare once, and rescore remaining choices against the current input. | Predicate substitution, partial coverage, negation, stale handles, scope and cost-reversal tests pass. |
| 4. Execution observations | Attach optional per-node measurements to the same execution that returns rows. | EXPLAIN does zero candidate-result work. ANALYZE passes the measured, skipped, cache-hit and multi-domain cases in R5. |
| 5. Ownership and module extraction | Remove the remaining result copies and transfer the actual algorithms to their specified modules. Separate mechanical moves from behavior changes in Git. | The ownership checks and module responsibility checks in R6–R7 pass. |
| 6. Integrated validation | Run differential tests and paired benchmarks, then correct the completion record. | Raw evidence identifies the exact final code revision and its comparison baseline. |

Read `AGENTS.md` and `docs/ai/README.md`. Use the existing sibling integration worktree under `/home/simon/source` if available. If the branch has advanced, keep its newer commits, recheck each finding by symbol, and reuse any repair that passes its gate. Keep the original checkout on `master`. Push each completed commit to the integration branch. Leave the merge to the user. Preserve the previous report and append a dated correction with links to the new evidence.

## 2. Gate A — one public query that exercises the architecture

Create a small collection with stable record IDs. Express the test using a public `Query` with field-scoped literal FTS for `rare` on `title` and structured FILTER `year = 1800`. Use locator output, exact count, deterministic order, disabled verified cache and no early LIMIT. The numbers below denote fixture record identities; resolve them to each domain’s actual slots when constructing expectations.

| Record ID | title | year |
|---|---|---:|
| 0 | rare find | 1799 |
| 1 | common | 1800 |
| 2 | ordinary | 1799 |
| 3 | rare | 1800 |
| 4 | ra ar re rxr axe | 1800 |
| 5 | common | 1799 |
| 6 | ordinary | 1800 |
| 7 | plain | 1799 |

The structured set is `{1,3,4,6}`. The exact text set is `{0,3}`. Record 4 supplies the adjacent and skip-one grams of `rare` separately; it exercises residual verification. Configure the independent text provider to return the conservative posting `{0,3,4}` for this request, calculated from its fixture index. Assert the exact final result is `{3}` with count 1.

- **Text first.** The first bulk operation takes eight slots and produces `{0,3,4}`. The structured operation must receive precisely those three slots and return `{3,4}`. Exact text verification eliminates 4.
- **Structured first.** The first operation takes eight slots and produces `{1,3,4,6}`. Text restriction must receive those four slots and return `{3,4}`. Final result and evidence agree with the first run.
- Use instrumentation on the provider boundary to record the effective input and returned slots in this small test. If the implementation validly fuses residual verification with restriction, also expose the candidate count before verification in the fixture recorder. Production diagnostics retain only counters.
- Change **advertised costs** to select the two orders. Both predicates must remain indexed in the controlled cases. Reverse registration order separately and confirm an unequal-cost winner stays the same. The fixture may install providers through a private repository construction hook. Query preparation, selection, application, verification, ordering and delivery must use ordinary production entry points. The test itself calls search and inspects observations; it never chooses or invokes the winning operation.
- Run with native providers, the independent sorted scalar/gram provider, and indexes disabled. Compare ordered hits, exact counts and requested evidence with the independent reference evaluator. Prove both FILTER and FTS providers are applied in the independent configuration. With indexes disabled, prove exact verification ran. Repeat against a published segment and a mixed pinned view with overlapping local slot numbers. Controlled candidate sets and costs apply to the fixture provider; native results must be semantically identical and native index use must be demonstrated on eligible data.

This gate detects a silent scan, a planner that ignores provider cost, a lost restriction and a gram false positive. A direct unit call to `propose` or `apply` covers the adapter only.

## 3. R1 — give membership one meaning in every representation

**Location:** `query_access/membership.rs`, especially `bitmap`, `same_members`, `is_subset`, `intersection_len`, `intersect`, `union`, `subtract` and `forward_slots`.

`SlotInput` with an empty result over `{1,2,3}` currently borrows the original input for some comparisons. Two empty results can compare unequal, and an empty result can report an overlap of three. The symbolic unrestricted case also refers to the supplied input. Two restrictions in the same domain can have different supplied inputs, so forwarding whichever input is longer can change membership.

1. Separate the **pinned slot domain** from the **effective members**. Define read-only `MembershipView` cases for empty, all slots in a specific active view, and a borrowed concrete bitmap; keep ownership in a separate non-`Clone` result owner. Resolve “unchanged” at the operation boundary by forwarding the exact input handle supplied to that operation. Do not let a symbolic value acquire another operand’s input during algebra. Remove the length-based choice in `forward_slots`.
2. Implement equality, subset, disjointness, overlap count, contains and ordered iteration on the effective view. Empty is a subset of every set, is disjoint from every set, and has zero intersection cardinality. Two sets are equal iff their members agree, including two differently represented empty sets. Use borrowed Roaring operations or ordered iteration. Comparisons allocate no result bitmap or boxed per-slot iterator.
3. Implement union/intersection/difference on domain-qualified results. Forward identity results, move a uniquely owned operand into in-place algebra where valid, and compute a genuinely different result once. Subtraction preserves operand order. Handle borrowed-left/owned-right efficiently for commutative operations. Apply full-domain shortcuts only when the operand actually denotes the canonical full active domain.
4. Replace `query_execution/domain.rs::domain_id`’s lossy XOR/truncation with a structured identity containing pinned repository instance/revision, collection, and base-or-segment identity at its actual width. Alternatively intern that full tuple within its pin and carry the pin identity alongside the interned ID. Equal local slots from different domains must be rejected before any comparison or combination. Two effective subsets from the same domain may be combined normally.

**Tests.** Use ordinary integer sets as the oracle. Over a four-slot domain enumerate all subset pairs, including empty, equal, disjoint, overlapping and nested. For each pair exercise every applicable symbolic, borrowed, owned and shared representation, both operand orders, and different narrowed inputs in the same domain. Compare all read operations and algebra. Specifically check `Empty == Empty`, `Empty != {1,2,3}` and `overlap(Empty,{1,2,3}) == 0`. Add different-collection, different-segment, new-revision and different-repository cases. Include a collision of the previous encoding, such as collection 0 with `(revision 0, segment 0)` and `(revision 1, segment 131072)`. Keep worker and pin lifetimes explicit.

## 4. R2 — each provider restricts precisely the survivors it receives

**Locations:** `query_access/{native,text,independent,contract,registry}.rs` and `executor.rs::segment_fts_candidates`. Today multiple providers call `input.input()`, which exposes the original bitmap. The current fixture hides this error by using predicates with identical final memberships.

Expose only the effective input to provider restriction operations. Replace `input.input()` with a typed borrowed view supporting `contains`, iteration, count and bulk intersection. Where an algorithm separately needs the whole logical/coverage domain, obtain that metadata from its pinned adapter and apply exclusion only in the covered part of the effective input. Ensure every result satisfies `proven ⊆ true matches ⊆ possible ⊆ effective input`. Return unchanged input, empty, exact, candidate-only, or distinct possible/proven bounds without duplicate sets.

Pass the actual survivors into field, raw and segment FTS. Remove whole-domain `(0..record_count).collect::<RoaringBitmap>()` construction followed by an intersection merely to satisfy the adapter signature. If a native index must read its entire posting, charge for that work and write its restriction into the final destination. For an indexed sorted backing, probe keys, intersect slot lists against the effective input and produce the requested output once. Derive estimates from stored cardinalities or bounded summaries. The current independent provider constructs matching slot lists in `propose` and repeats the work in `apply`; remove that pattern.

A selected handle belongs to one provider instance, a particular prepared operation/term and its pinned domain. `NativeCollectionProvider::propose` currently clears its operation list, so a later proposal can reuse an earlier ID. Use request-owned stable operation slots and check provider identity, term and domain on application. Register distinct instances unambiguously, including two backings for the same access family. An unsupported capability yields no offer. Propagate checked storage, corruption, stale handle, cancellation and budget failures using existing error semantics. Remove `.ok()?` conversions that disguise a selected access failure as an unavailable index.

**Tests.** Effective input `{3}` with underlying posting `{1,3}` returns `{3}` for native and independent providers; repeat with empty input. Offer A, then B, then apply A. It must still execute A. Apply A in the wrong term/domain and expect a checked error. Inject a failing *selected* apply through public search. Verify zero candidate allocations and zero apply calls on estimate-only paths. Retain pinned views across publication and verify old requests continue to see their own domains.

## 5. R3 — make the selected access executable through the production catalog

**Locations:** `executor.rs::PlanContext::{paths,term_id}`, `rescore_paths`, `plan_logical_expr`, `segment_expr_bounds`, `query_plan/selection.rs`, `query_access/registry.rs` and the top-level FILTER/FTS ordering in `query_execution/runner.rs`.

1. Assemble one catalog for each pinned execution domain at request construction, including the native services that are available there. Tests may substitute or add conforming providers at this same construction site. A prepared leaf asks its catalog for eligible offers. Keep a selected opaque handle, its semantic guarantees, and its cost/coverage estimate in the physical plan. Execute that exact handle through the same catalog. Move `execute_path` and concrete native kernels behind the adapters. Remove the path that converts a selected offer back into native `AccessPathEstimate` and dispatches in the executor.
2. Use the same selection function for base and segment data, structured FILTER and FTS, and competing safe algorithms. Cover typed/structural/inferred/named postings, metadata/file paths, sort-column restrictions, field/raw/fallback text, regex necessary conditions and relation alternatives at their correct scope. Preserve optimized ordered traversal and relation joins through explicit plan operations. When a domain lacks an index, residual verification remains available. Different domains may choose different plans.
3. Cache stable offer data by domain and syntactic `TermId`. Rescore all still-eligible alternatives at the current input cardinality and choose the new minimum. A cache hit must not select `paths[0]` from the old ranking. Implement one deterministic tie rule. Add a cost crossover test: two equally valid paths cost `2 + N` and `20` in comparable units, with identical remaining verification costs. At `N=100` choose the second, and at `N=3` choose the first. Establish the changed cardinality via a preceding restriction in production execution. Confirm the losing provider receives no apply call.
4. Prevent per-term full-domain bitmap creation, rebuilding a native path vector after selection, cloning/sorting all alternatives for ordinary execution, and repeating dictionary estimation during residual ordering. Keep the complete alternatives list where adaptive planning or EXPLAIN needs it. Record dictionary visits and verify estimate work remains bounded.
5. Support partial coverage deliberately. Preserve uncovered eligible rows as possible matches, take the union across alternative FTS fields/fallback populations, and retain exact verification for gram survivors. One unrestricted regex alternation branch prevents exclusion of that branch. Under NOT, invert proofs only within a known two-valued scope; retain unknown source/relation outcomes for the exact evaluator. Membership proof and matched-field evidence have separate obligations.

**Acceptance.** Gate A passes. Also test a partially covered matching slot, raw-invalid fallback, one uncovered alternative field, a regex with an unrestricted alternative, and an unavailable relation target under NOT. Check results against the reference scan. Test execution can replace a backing without editing optimizer or runner files.

## 6. R4 — hide filter implementation details and bind each occurrence once

**Locations:** `prepared.rs`, `query_predicate/mod.rs`, `query_execution/runner.rs::{DirectSourcePlan,Residual,prepare_residual}` and `query_access/contract.rs::AccessRequest`.

Make the optimizer-facing leaf interface small. The Boolean tree contains opaque leaf IDs. The optimizer can ask for eligible offers, semantic ordering constraints and estimated cost. The runner can apply the selected bulk operation, verify an unresolved record, and request input/evidence requirements. It sees restrictions, truth values and measurements. A concrete filter owns bound-term matching, transforms, array semantics, regex, reader/representation compatibility and how to produce evidence. Split predicate implementations into `structured.rs`, `text.rs` and `relation.rs` behind this façade. Retain the existing exact matchers, registered binding operations and independent reference evaluation.

Store prepared leaf state once in an indexed request-owned table; each collection holds its expression and leaf IDs. The reviewed code stores bound expressions, clones leaves into a collection table, then clones them again into the request table. Remove these duplicate ownership paths. Use IDs directly for estimates, chosen operations and proofs. Source spans identify text for diagnostics. Two independently constructed terms can have the same span; they must still get distinct plans and observations. Compile projection and evidence requirements once. Give the scan worker the combined input requirements, including sort and relation requirements where applicable.

Move reader-specific direct scalar eligibility behind an internal capability or a conservative default on the existing core reader interface. Dispatch once during preparation. Remove checks such as `collection.source.id() == "json_decoded_scalars"` from the runner. A replacement exact algorithm with the same semantics must not require edits to the runner or optimizer. Ensure `Truth::Unknown` can actually be returned for unavailable data where applicable. Preserve original error priority, evidence under OR/NOT, and the bounded source-scan fast path.

Pass the prepared request and frozen clock into regular execution, lifecycle recovery, cache-miss execution, ordering and diagnostics. Preserve existing cache-hit validation behavior. A second bind to render an estimate or retry a recoverable source read is unnecessary. Use test-only binding counters to detect it.

**Acceptance.** Execute the same field predicate with two independent exact implementations through public search on a fixture with unproven survivors, evidence enabled, AND and OR. Verify both implementations ran on their respective requests and return identical ordered results/evidence. Add two same-span terms with different fields, a frozen-clock query, and an unavailable-source case. Search the runner and optimizer for direct bound-leaf inspection and reader-ID selection. Their only leaf access should be the façade’s permitted operations.

## 7. R5 — record EXPLAIN ANALYZE where operations run

**Locations:** `query_execution/runner.rs::explain_segmented` and `search_analyze_segmented`, notably the loop at lines 8349–8358 of the reviewed revision; `query_diagnostics/trace.rs`.

- Build the initial plan once. `EXPLAIN` inspects metadata and bounded estimates under the same selection policy. Avoid `active.bitmap()` materialization and temporary bitmap intersections merely to compute a displayed estimate. Render missing statistics as unknown or estimated. Keep candidate builds, source reads and apply calls at zero.
- `EXPLAIN ANALYZE` runs exactly one instrumented execution, with the same request settings, cache policy and engine as search. Attach a node ID, term occurrence ID(s), collection/domain and chosen operation to each observation. Count effective input immediately before application and candidate/proven output immediately after. Track residual verifications separately from prefilter survivors and final rows. Keep observations at physical-node scope when a fused operation serves multiple terms.
- Use worker-local observations and merge once. An ordinary search needs no per-record trace allocation, locking or atomic increments. A plain cache hit emits the cache-hit observation; it executes no filter operators. Preserve genuine measured zero, skipped, estimated and unavailable as distinguishable states under the existing public trace contract. A node that examined rows and returned zero was executed.
- Remove the post-search explain pass and the code that infers scanner, bigram, posting, ordering or relation strategy from aggregate counters. Also remove the loop that copies `trace.input_records` and `trace.candidates` into every term. Report observations for all collections whether there is one trace or several.

**Acceptance.** Check Gate A’s distinct node counts in both orders. Check an AND with an empty first result: first term executed with output zero, second term skipped. Test a deliberately scanning text predicate, a false-positive gram survivor, a fused range, two collections and a cache hit. Capture actual apply counters independently from trace rendering and compare them with the trace. Ordinary EXPLAIN must leave all provider application and source counters at zero, including native shortcuts outside the registry.

## 8. R6 — eliminate copies of existing result data through delivery

The invariant applies to candidate/proof bitmaps, completed membership, evidence, rows, provisional pages and payloads. Borrow an immutable result, move an owned allocation, or share a pinned immutable owner. Construct a genuinely new intersection, union or caller-owned output once. A handle, scalar slot or `Arc::clone` transfers identity without copying the underlying data.

| Site on the reviewed revision | Required repair |
|---|---|
| `executor.rs::segment_expr_bounds` clones exact sets for `may` and `must`. `plan_logical_expr` clones input and matches when evidence is enabled. | Represent an exact restriction with one owner. Refer to original input scope and match set by stable handles for as long as an evidence consumer needs them. Keep positive proof separate from the ability to emit field evidence. |
| `repository.rs` around 1438 and 1454 calls `rows.to_vec()` to stamp both native stream event types. | Stamp revision/identity once at the final owning producer. Lend that same canonical slice to the synchronous sink. Compare streamed IDs and revision stamps against synchronous hits. |
| `emit_provisional_page` clones every visible winner; replacement detection clones collection and record ID strings. | Keep heap/order entries as compact domain-qualified row keys. Move visible owned hits into one reusable contiguous delivery buffer, lend the slice, and move surviving hits back to their owners. Preserve buffered payloads across replacements. Detect page changes by stable row keys and order. Handle sink failure and cancellation without retaining two pages. |
| Cached result materialization clones `member.matched_fields`. `materialize_cached_stream` repeatedly sums the growing page’s byte size. | Keep cache evidence as compact references into immutable revision-scoped field metadata. Construct public evidence once for each requested output row. Maintain checked running live-byte totals as rows move into/out of the page. Keep the existing `VerifiedCache::insert_shared` handoff and compact `SourceHitLocator` recovery. |
| Whole-domain construction, whole-input clones and collection of unchanged membership still occur in executor/runner paths. | Forward the effective owner on identity results. Use a symbolic all-active view or borrow the pinned active domain; build a contiguous active bitmap once on demand if required by a native kernel. Carry its handle through the request. Keep worker scratch reusable and budgeted. |

**Acceptance.** Verify buffer/owner identity for a borrowed exact result, its proof, an identity restriction, cache publication, native stream stamping and a large unchanged payload between consecutive provisional pages. Verify zero temporary result allocations for equality/subset/overlap comparisons. Force an evidence-enabled OR, cancellation during a provisional page and cache eviction with an outstanding reader. Audit `clone`, `to_vec`, `to_owned`, `cloned().collect()`, copy-on-write and equivalent rebuild loops in the migrated production path. Pair instrumentation with code inspection; pointer equality before and after a call alone cannot detect a transient copy. Preserve existing public owned-return types. Implement caching of metadata/handles and final output ownership accordingly.

## 9. R7 — split the actual runner by responsibility

The reviewed `run.rs` is a façade, while `query_execution/runner.rs` still has 13,481 lines. `query_results/{ordered,delivery,materialize}.rs` each contain one comment. Use the following ownership map to finish the extraction. All paths are under `crates/core/src/`.

| Owner | Functions and logic to transfer |
|---|---|
| `query_execution/request.rs` and `domain.rs` | Typed prepared request, frozen clock, pinned domain iteration, cache lookup/recovery coordination and budget/cancellation context. Decompose `search_segmented_impl` and `search_impl_with_collector` into prepare/select, execute domains, finalize. |
| `query_plan/{selection,boolean,ordering}.rs` | Cost comparison, rescore, legal Boolean order, combined FILTER/FTS choice, traversal selection and early-stop eligibility. Replace identity-order placeholders with the shared policy called in production. |
| `query_predicate/{structured,text,relation}.rs` | Leaf verification, signed restrictions, evidence, source projections, direct scalar/FTS eligibility and relation truth. `prepare_residual` takes prepared term IDs, proof handles and existing costs. |
| `query_access/{native,text,metadata,posting,relations}.rs` | Storage-specific proposals and application, including base/segment differences, typed/structural/positional access, ID/file restrictions, range fusion, column access, grams and semijoin capabilities. Delegate to existing native data structures. |
| `query_execution/{source_scan,segment_scan,relations}.rs` | File/range grouping, source access sessions, worker jobs, `scan_file`, `verify_local_segment_record`, parallel scanning and relation page execution. Refactor the file scan into borrowed verification program, file/ranges job and explicit result/budget state. Existing top-level pre-index source scanning keeps its own API. |
| `query_results/{ranking,ordered,cache,materialize,delivery}.rs` | Winners, sort keys, ordered cursors and merge; `try_stored_order_page`, inferred and lifecycle ordered traversal; verified cache admission/replay; payload creation; `emit_provisional_page`, batch delivery and replacement. Preserve bounded top-k and early stopping when count/output rules permit them. |
| `query_diagnostics/trace.rs` | Render plan data and actual observations, including relation decisions and per-domain totals. Contains no source reads, index probes or heuristic reconstruction of execution. |
| `run.rs`, `executor.rs` and `query_execution/runner.rs` | Keep existing public paths through re-exports. The remaining coordinator calls the stage owners. Each former specialized algorithm must live with its owner and have a production caller there. Remove empty shells, obsolete callers and unused parallel implementations. |

Use separate move-only commits where helpful. Existing index, relation, sorting, source and streaming primitives can stay in their current modules. Do not duplicate those kernels just to populate directories. Acceptance is based on ownership: a change to text verification, provider storage, cost policy, ranking, stream emission or trace rendering has a specific module to edit. A central file still owning scanning, storage selection and page delivery together fails the gate regardless of its filename or line count.

## 10. R8 — core-only verification and performance evidence

Run only the core crate’s build, tests, examples and benchmarks for this assignment. Use the pinned repository toolchain, lockfile and the same Musenalm corpus, schema, worker counts, alphabet and cache settings for each comparison. Invoke the existing core benchmark from the repository root.

```sh
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

Run the following benchmark commands in both the baseline and final worktrees, each using its own release binary. Set `MUSENALM_ROOT` to the shared corpus, and `RESULT_DIR` to a fresh path for the **current revision**. These options exist on the reviewed core example.

```sh
./target/release/examples/bench_musenalm \
  --root "$MUSENALM_ROOT" --extended \
  --cases Q001,Q007,Q030,Q035,Q084,Q127,Q141,Q150,Q163,Q164,Q181 \
  --profiles ascii-off --query-workers 4 --repetitions 5 \
  --result-dir "$RESULT_DIR/ascii-off"

./target/release/examples/bench_musenalm \
  --root "$MUSENALM_ROOT" --extended --incremental \
  --cases Q030,Q141,Q163 --profiles unicode-off,unicode-warm \
  --query-workers 4 --repetitions 5 \
  --result-dir "$RESULT_DIR/incremental-unicode"
```

Use original master `2e18871c0b` as the merge comparison. The reviewed branch tip is the starting point for repair attribution. Reuse comparable baseline raw samples when available; obtain missing master samples once. Run only a focused Q030/Q150 check on the reviewed tip if needed to locate a regression. During edits use the small deterministic fixtures and affected cases. Reserve full paired runs for the integrated code. Keep cold-cache, warm verified-cache and ordinary search measurements distinct from diagnostic time.

Save individual samples, medians, worker/cache settings, compiler, machine, commands, case IDs, revisions, corpus identity, result digests and work counters. Compare candidates, applied operations, file opens, bytes, parses, residual checks, evidence, first-page behavior and actual allocations/copies on the focused fixtures. Measure first-page behavior using core stream tests or a small core harness if the Musenalm example lacks that timing. Investigate a slowdown exceeding both 5% and 0.2 ms by alternating only that case in base/final order. Report consistent changes below that threshold accurately. Previously Q030 and Q150 were slower in both rechecks; distinguish a directionally consistent sub-threshold difference from a demonstrated cause. A new full scan, extra whole-domain pass, deep result copy or wrong result requires investigation regardless of elapsed time.

### Definition of done

Append an implementation report to this repair task. Preserve the previous report as history and identify each statement superseded by these results. Put requested evidence under `docs/ai/generated/query-execution-refactoring/repairs/` and move the completed task document into `docs/ai/done/` according to repository workflow. Those files document the core work.

| Required evidence | Reviewer must be able to verify |
|---|---|
| R1–R2 counterexamples and set-oracle tests | Empty, restricted, negative, stale and cross-domain cases behave correctly; an eliminated record cannot reappear. |
| R3–R4 production call paths | Public core search selects either backing and either operation order; exact matcher substitution works; neither optimizer nor runner inspects filter/storage implementation details. |
| R5 executed trace | Real per-node inputs/outputs for both Gate A orders, an executed zero, a skip and a cache hit, plus distinct observations for multiple domains. |
| R6 ownership table | Each named copy site has a final source location, replacement ownership path and test/measurement. Existing shared-cache and locator improvements remain effective. |
| R7 responsibility map | Each named algorithm has an implementation owner and an actual production caller. Include before/after file sizes for context. |
| R8 paired evidence | Core test outcomes, raw benchmark samples and counters, both incremental Unicode profiles, and any unresolved regression with its measured impact. |

Record the final implementation commit separately from the report commit. Mark a requirement pending if only an isolated helper, a manual provider sequence or an unconnected module implements it. Deliver the integration branch and its evidence for the user’s merge decision.

### Reviewed source anchors

## Implementation report

- Task: `core-query-execution-repair`
- Completion date: 18 November 2026
- Base commit: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- Integration branch: `integration/query-execution-refactoring`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-refactoring`

### Summary

The correction pass closes the remaining provider routing, ownership and module
responsibility gaps from the `3daa33e381` checkpoint.

- Relation target-first and source-restricted alternatives are now provider
  offers in `query_access/relations.rs`; the selected stable handle executes
  through the production catalog. Whole-page relation fallbacks are ordered by
  an explicit `query_plan/operations.rs` plan.
- Fused FILTER/FTS source execution records one physical observation with all
  participating term IDs only when FILTER actually survives to source
  execution. ANALYSIS observations are attached by collection identity, fixing
  queries that select a nonzero collection while another collection is
  unselected.
- Exact possible/proven segment bounds share one immutable bitmap owner.
  Verified-cache evidence is held as shared `Arc<[String]>`, and public evidence
  vectors are constructed only for requested output rows.
- Ranking and sort policy, cache replay/materialization, trace rendering,
  structured proof evidence, and residual/source-projection planning have
  dedicated owner modules. The coordinator is 6,770 lines, down from 8,318 at
  the start of this correction pass.

### Commits

- `9e0a49bf27` Route relation plans through providers and compact cache evidence
- `3f25e432f1` Extract ranking trace and predicate occurrence ownership
- `e80087d24f` Extract structured proof evidence from coordinator
- `f9174d5a53` Move residual and source projection planning to predicates
- `0d4ed9df14` Verify exact segment bounds share one owner
- `2402cb6746` Attach execution observations by collection identity
- `f0df3ba685` Reuse shared segment bounds through provider application

### Validation

```text
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

All three commands passed at final implementation revision
`f0df3ba68548a4e487556b3cbfcd7f0fba2db590`. The core run passed 564 library
tests, all 28 integration tests and the doc test.

### Performance

Paired Musenalm samples, reports and manifests are under
`docs/ai/generated/query-execution-refactoring/repairs/final-f0df3ba6/`.
This includes raw base/final five-sample matrices, Q030 and Q141 focused
100-repetition reruns, and `perf stat` output. All result digests and ordinary
work counters match original master.

The balanced ascii-off run has no case crossing both the 5% and 0.2 ms gate.
Q001 exceeded 0.2 ms without exceeding 5%; Q084 exceeded 5% without exceeding
0.2 ms. Q007, Q030, Q141, Q150, Q163, Q164 and Q181 measured faster than base.

The five-sample incremental Unicode matrix showed substantial host variance.
Focused 100-repetition Q030 measured +0.034 ms and is effectively neutral.
Q141 measured +0.336 ms in final-first order and +0.173 ms in base-first order;
`perf stat` measured +0.19 ms per complete query over 100 iterations. Candidate,
scan, parse, residual and result work remain identical, and no extra
whole-domain pass or result difference was found.

The progress record and detailed evidence summary are preserved at:

- `docs/ai/generated/query-execution-refactoring/repairs/progress.md`
- `docs/ai/generated/query-execution-refactoring/repairs/final-f0df3ba6/REPORT.md`

### Cleanup

No subordinate worktree was created for this correction pass. The integration
branch and worktree are retained for the user's merge decision.

[Membership comparison and algebra](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/query_access/membership.rs#L103-L294) · [Native provider proposals and handles](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/query_access/native.rs#L111-L199) · [Structured execution](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/executor.rs#L2381-L2421) · [Independent provider fixture](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/query_access/independent.rs#L335-L443) · [Segmented ANALYZE](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/query_execution/runner.rs#L8222-L8359) · [Provisional page ownership](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/crates/core/src/query_execution/runner.rs#L12376-L12454) · [Prior benchmark report](https://github.com/Simon-Martens/objekte/blob/20e17c5be0d1f6c227503d5681c2bf7c76ca2808/docs/ai/generated/query-execution-refactoring/results.md)
