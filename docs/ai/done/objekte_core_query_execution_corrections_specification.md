# Objekte core query execution — production-path corrections

**Assignment for the local implementation agent · 18 November 2026**

Make the production query engines choose interchangeable indexes and exact matchers for FILTER and FTS. Carry candidates, proofs, evidence and completed rows through execution without copying an existing result. A caller must get the same records and count when evidence is disabled, when the index representation changes, and when a query spans several published segments.

**Starting revision.** Use `integration/query-execution-refactoring` at `74e569f505b8059aba1befef517c0c58e1e87bc0`. Its final code revision is `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`. The following anchors refer to that tree. The previous repair specification is in `docs/ai/done/objekte_core_query_execution_repair_specification.md`. Its acceptance tests remain applicable.

**Delivery.** Create the follow-up branch `integration/core-query-execution-corrections` from the reviewed integration tip, in the sibling worktree `/home/simon/source/objekte-integration-core-query-execution-corrections`. If that integration tip has advanced, start from its descendant, record the SHA and run the reproductions first. Follow `docs/ai/README.md` for task records, commits and pushes. Leave the original checkout and the master merge to the owner. Implementation, tests and benchmark code belong in `crates/core`. Put the final evidence under `docs/ai/generated/core-query-execution-corrections/`. Preserve the public Rust query, result, streaming and extension APIs and existing index formats.

## 1. Establish the failures before refactoring

Extend the eight-record fixture in `crates/core/src/run.test.rs::gate_a_public_search_selects_either_provider_in_either_order`. Use filename identities `0` through `7` in deterministic order and the exact title/year values already present. Record `3` has title `rare`. Record `4` has `ra ar re rxr axe`, which contains the necessary grams in separate positions. Both have year 1800. Use a field-qualified, case-sensitive literal FTS query for `rare` in `title` and a structured `year = 1800` term. Declare the field types explicitly. Disable the verified cache for initial execution.

| Operation | Expected slots in the original fixture |
| --- | --- |
| Title gram candidate restriction on the entire fixture | `{0,3,4}` |
| Exact year restriction on the entire fixture | `{1,3,4,6}` |
| Either restriction applied to the survivors of the other | `{3,4}` |
| After exact verification of the complete query | `{3}` |

Use the public `Repository::search`, `count_exact` and an ordered stream. Check ordered IDs, count and evidence against `search_reference`. Toggle evidence. Repeat `count_exact` with evidence disabled, which exercises a zero-limit query. A single native published segment with a proven year match and a candidate-only text match must return exactly one. Exercise a second fixture partitioning the same records across two segments whose local slot numbers overlap. Compare stable record IDs across publications. Include an update or deletion so an obsolete matching slot stays physically present and is invisible to the query.

Run three configurations over the **same published data and normal dispatch**: native access, the independent sorted-slot provider, and source verification with eligible index offers suppressed for this request. Dropping the test registration on an indexed repository restores native indexes and is insufficient to establish scan-only behavior. Record selected engine, provider/operation, domain, effective input, output, proof count and residual count through test instrumentation. Verify that each configuration actually ran. Force both legal orders by changing positive provider costs while holding the query and data fixed. Where the native gram configuration admits additional false positives, use a known deterministic gram configuration for the exact intermediate-set assertion and require the independent oracle to verify final results for every configuration.

## 2. Correct proof composition on the segment path first

**Defect.** `query_execution/runner.rs` around lines 2102–2128 constructs FILTER bounds, computes FTS candidates separately and intersects them. Lines 2160–2179 then treat survivors in `bounds.must` as final matches when evidence is disabled. A structured proof establishes only the year. The code increments the count, may admit the row to the cache or page, and removes it from the set requiring verification at line 2239. This condition is also present on the original comparison branch. Matching benchmark digests across branches cannot detect it.

1. Introduce a focused correctness commit. Gate the proven-row shortcut on proof of **every required predicate**, including FTS. For ordinary gram access, pass the proven FILTER result to the verifier as a leaf proof and still verify FTS. With unavailable FTS access, verify the text over every surviving slot. With exact metadata FTS, combine its actual proof. Ensure exact count, top-k, streaming, cache admission and cache replay all use the same final-membership rule.
2. Model each leaf result as possible membership and proven-true membership within its effective input and snapshot. Keep unresolved slots available to the exact matcher. Compute conjunction, disjunction and negation using the existing three-valued query semantics. In particular, an unavailable source under NOT cannot become a true record just because the positive term lacks proof. Preserve OR evidence when several branches match and the existing error/short-circuit order for fallible bindings.
3. Add two targeted cases besides the eight-record test. One has a text literal with no usable index restriction and a selective exact structured term. The other warms the cache with evidence disabled and compares cold and warm IDs/counts with the reference, then checks the evidence-enabled request independently. Exercise sorted page and count-only entry points separately so an early-return optimization cannot hide the failing path.

**Pass condition.** No code path that observes only a FILTER proof can publish, count or cache record `4` as a completed FTS match. With evidence on, record `3` retains both field names.

## 3. Give every occurrence one identity and every domain its full identity

**Defect.** `query_predicate/scan.rs::prepare_residual` resolves a leaf by span alone. `executor.rs::PlanContext::term_id` and `SegmentPlanContext::term_id` use keys containing span and field path. Different leaves may legally share both. The prepared base path can apply the first leaf's request for the second occurrence. Segment text also uses the constant `TermId::new(0)` and the segment creates another counter starting at zero. The committed Q141 trace uses term 0 for both `year` and text.

- In `prepared.rs`, allocate `TermId` once per syntactic occurrence per bound collection, including FTS. Move the bound implementation into one request-owned leaf table. Store only the Boolean shape, occurrence IDs, FTS ID and collection metadata in `PreparedCollectionQuery`. Eliminate `leaves.extend(collection_leaves.iter().cloned())` and duplicate bound expressions after their callers have migrated. Use indexed lookup for dense IDs. A source span and display path are diagnostic attributes.
- Pass the prepared expression and actual `TermId` into offer enumeration, physical operations, proof association, residual execution, relation inputs, skipped observations and trace rendering. Remove `term_for_span`, occurrence lookup by `(span,path)`, local ID generation and synthetic text ID zero from these paths. Preserve a separate physical operation ID when a fused expression covers multiple leaf IDs.
- Create the domain at snapshot acquisition with the actual repository instance, collection ID, pinned visible revision and full-width base/segment identity. Segment structured and text constructors currently supply repository `0`. Carry the domain through adapters, cache keys where applicable and observations. A physical segment that survives a visibility revision can still have a different active set.
- Build the public query programmatically. Give two terms the same span and different fields, then the same span and the same field with different literals. Use `year = 1800 AND year = 1799` as the latter case. Exercise both exact-index and residual-only routes and repeat for OR and evidence. On a combined FILTER/FTS request collect distinct leaf IDs. Compare operations across two repositories, revisions and segments containing the same numeric slot, and assert a typed domain mismatch.

## 4. Make the production planner and leaf boundary interchangeable

**Defect.** `repository.rs::search` and `explain_analyze` contain a test-only engine switch. A registered provider or alternate matcher can force the base engine for a repository that normally uses segment execution. The segmented runner applies FILTER first and asks FTS to inspect all physical slots. `executor.rs::segment_fts_candidates` creates that domain and rejects borrowed or shared output. `NativeSegmentProvider::propose` advertises a generic `SourceScan` at a fixed cost while `segment_expr_bounds` chooses the physical access inside `apply`. A new backing or matcher cannot rely on this surface.

**Implement the following call sequence for searches, counts, streams and diagnostics.**

1. **Prepare once.** Resolve the clock and parameters, pin the snapshot and compile the predicate table, scan projection, sort/relation input requirements and evidence policy. Pass the prepared request into ordered traversal, fallback after a recoverable source read and diagnostics. `try_lifecycle_ordered_page` currently calls `prepare` after `search_segmented_impl` has already prepared. Record per-request bind counts for ordinary, cached, ordered and analyzed execution.
2. **Propose without executing.** At each domain assemble one internal catalog of eligible native and registered capabilities. Test configuration can add an adapter or suppress offers at this point. Delete `use_test_provider_pipeline` from public dispatch. Present separate executable offers for compatible postings, ordered columns, text grams, file/metadata and relation choices. Include the existing source-verification fallback. Each offer carries opaque provider and operation handles, term IDs, coverage and proof guarantees, estimated cardinality, startup and per-input work and applicable order/evidence properties. Preserve specialized native kernels and fused range lookup behind their owning adapter.
3. **Select from actual costs.** The shared Boolean planner compares eligible structured and FTS alternatives across providers and reconsiders remaining legal choices after a reduction. Reuse the existing legal-reordering constraints. Store native cost components separately. `query_access/native.rs` currently puts the entire `path.estimated_work` in startup and supplies zero input cost, which prevents rescoring a cached native offer when input shrinks. For input-dependent statistics whose cost cannot be represented linearly, carry a cheap provider estimate keyed to effective input size. Use deterministic tie breaking. An independent test with costs `2 + N` and `20` must choose the constant offer at 100 survivors and the other at 4 survivors regardless of registration order.
4. **Apply the selection.** Execute the winning prepared handle with the actual survivor view, pinned domain, cancellation token and budget. FTS following FILTER gets four slots in the eight-record test. Skip the second operation entirely after an empty result. Apply an AND child on the narrowed domain, an OR or NOT child on its semantically correct input. Accept borrowed, moved, shared, empty and identity results uniformly. Check provider outputs and proofs against the effective input in conformance tests and checked boundary validation. Keep hot-path validation constant-time where ownership or construction already establishes the invariant.
5. **Honor effective input and failures.** `NativeSegmentProvider::apply` reads `operation_input()` rather than effective membership. The collection provider can turn a no-restriction response into `replace_slots(Unrestricted)`, which can also widen an already narrowed input. Forward the incoming owner on identity. Apply a test restriction with original slots `{0,1,2,3}`, effective slots `{1,3}`, and an index containing `{0,1,2}`. The output is `{1}`. Test an identity operation on the same narrowed input. Handle a provider capability withdrawal by a semantically complete alternative when supported. Propagate cancellation, resource exhaustion, corrupt data and invalid operation handles with their actual error category. Replace error-discarding `.ok()?` conversions and `AccessExecution { context: None }` at context-aware call sites. Limit source-recovery fallback to recoverable source failures. An unrelated unavailable file must not turn a corrupt-index error into successful search.
6. **Verify through the leaf.** Expose only semantic ordering constraints, input/evidence requirements, bulk restriction and exact verification with `True`, `False` or `Unknown`. Store a matcher selection at preparation and dispatch to it from both source-scan paths. Move raw `BoundTerm` inspection, array/transform rules and reader-name comparisons out of the runner, Boolean planner and `query_execution/segment_scan.rs`. Bind direct scalar scanning through an internal reader capability with a conservative fallback. Let the matcher write evidence directly to the requested sink. Avoid per-row temporary field strings or sets when evidence is disabled. Implement a test-only independently written exact matcher returning the same results and evidence for AND, OR and unavailable input. Inject it by request or repository and prove its invocation through **native segmented search** as well as base execution.
7. **Keep opaque results until a consumer needs rows.** The optimizer can ask cardinality, equality, subset, intersection count or disjointness. Its choice cannot depend on a concrete posting tree, gram dictionary, Roaring container or adapter name. The provider can borrow a resident bitmap, move an owned result or yield a pinned shared bitmap. The independent test adapter uses its sorted slot-ID backing internally and can return agreed bitmap membership. No public provider registration API is required.

## 5. Enforce ownership and correct resource accounting

| Current site under `crates/core/src` | Required change |
| --- | --- |
| `query_access/membership.rs::SlotInput::effective`, `MembershipView::{same_members,is_subset}` | Canonicalize `Unrestricted` on a zero-length input to empty semantics. Today it compares unequal to explicit empty and fails subset in one direction. Test all five representations over an actually empty backing, both operand orders, and the existing nonempty/narrowed oracles. |
| `executor.rs::shared_bitmap_from_slot_input`, `segment_expr_bounds_with_context`, `plan_logical_expr` | Consume an owned result into its canonical owner and share immutable results. Carry the child owner forward through AND. Remove `domain.clone()` initializers for identity, `child.may.as_ref().clone()`, and `Proof { input: input.clone(), matches: set.clone() }`. Base `proven_hits`, candidates and evidence for an exact result refer to one owner. |
| `query_execution/runner.rs::search_segmented_impl` and `executor.rs::segment_fts_candidates` | Forward FILTER membership directly into FTS. Borrow the active snapshot view or materialize it once per pinned domain if a kernel requires a concrete bitmap. Remove repeated physical-domain construction and repeated logical/fallback partitions when a child does not consume them. Forward unchanged input by identity. |
| `repository.rs` streaming remap closures, `runner.rs::try_lifecycle_ordered_page` | Stamp the pinned revision and remap diagnostics once while the producer owns each row. The synchronous result collector takes owned hits. The external synchronous sink borrows the same completed slice. Remove `rows.to_vec()` in both stream event variants and `hits.extend(rows.iter().cloned())` in the internal ordered collector. Cover a mixed batch in which just one row requires diagnostic remapping. |
| `query_results/delivery.rs::emit_provisional_page`, `streaming.rs::Delivery::replacement` | Preserve row bodies as winners move between heap and reusable delivery buffer. Track previous page identities by compact domain-qualified row keys. Remove `projected_copy` charging for moved rows and the repeated charge of `delivered` plus `event_bytes` for the same borrowed slice. Count each live allocation once, including scratch capacity, new payload bytes and retained identity keys. Enforce the event-size limit independently. Recompute retained-byte ownership when a previously empty payload becomes materialized. |
| `query_results/cache.rs`, `ordered.rs`, `verified.rs` | Keep cache membership, proof and evidence revision-scoped and shared. Build evidence using field IDs or shared immutable metadata and materialize the requested public row once. Preserve verified-cache eviction safety and incremental page-byte accounting. Audit all cache replay and ordered page paths, including fallback, for transient copies of an already-built row, bitmap or payload. |

**Ownership tests.** Check owner identity across a borrowed exact result, proof, unchanged intersection, cache publication/eviction and stream stamping. Instrument allocations or explicit full-result copy sites in addition to end-to-end identity checks. Test a large unchanged payload through two provisional pages and a payload added during materialization. A byte limit that fits one actual page plus scratch must succeed. A limit below the measured live allocation must return the existing resource error. Exercise cancellation and sink failure after a replacement. A genuinely new intersection or final owned public output is constructed once. Recording a second copy as scratch does not satisfy this contract.

## 6. Repair EXPLAIN and EXPLAIN ANALYZE with measured, request-local data

The committed `repairs/final-f0df3ba6/incremental-unicode/query-plans.jsonl`, relative to `docs/ai/generated/query-execution-refactoring/`, provides a regression example. Q030 unicode-off has 2,251 candidates and 1,264 matches. Its plan reports 1,264 actual candidates. Q141 assigns term 0 to both a year operation and text and records the three operations twice on the same physical `Segment(11)`. Inspect whether the repeats arise from ordered-page and exact-count passes. Preserve real repeat observations and label their phases. Share prepared restrictions between those passes where scope and inputs agree. Record how many physical applications the implementation actually removed.

- `EXPLAIN` calls the same selection logic with bounded estimates. Index application, whole-domain bitmap builds and source reads must stay at zero. Remove counters synthesized with `.max(1)` in `query_execution/relations.rs::explain_segmented`. An estimate can name an anticipated traversal while remaining explicitly estimated.
- Pass an optional request-owned observation collector through the normal execution context. A disabled collector returns before formatting a domain string, allocating `term_ids`, constructing an observation or consulting a registry. Remove global capture discovery and fallback to the sole active capture in `query_execution/request.rs`. An ordinary concurrent search must never contribute to an analyzed query. Use request-local worker buffers and merge once with stable request/domain/physical-node identity.
- Attach each observation by real collection, domain, physical operation, phase and prepared term ID(s). Attach cache-hit observations to the cache operation. Remove the unmatched-observation fallback to the only term, strategy inference based on index presence, and provider-number or priority rules for guessing which index executed. Report the actual kernel selected, including scanner fallback after an ineligible positional lookup. Distinguish executed with zero output, skipped, unavailable and estimated.
- In `search_analyze_segmented`, record the candidate count at the input to residual verification or the agreed candidate stage used by ordinary `WorkCounters`. Keep final matches and proven matches separate. Track work for every selected collection and publish its own `PlanActual`. A query-wide total assigned to the first collection is incorrect. For repeated passes retain separate node observations. Sum disjoint domains for collection totals and count unique candidate records according to the existing public counter semantics. Taking the maximum across a collection's segment nodes is insufficient.
- Compare `search` and `explain_analyze` using identical settings and fresh/warm cache state. Analyze runs one request with one prepared plan. Test two concurrent analyzes on the same repository, an ordinary concurrent search, two repositories with the same collection/segment numbers, a query selecting only the second collection, two selected collections, a true cache hit, empty short-circuit and a fused range. Use barriers and deterministic fixtures. Compare captured operation counts and domain identities with independently instrumented providers. An ordinary search must allocate zero trace records or strings.

## 7. Finish the module split at the production call sites

Retain working extractions in `query_results/{ranking,ordered,cache,materialize,delivery}.rs`, `query_predicate/scan.rs` and the source/segment scanning modules. The coordinator still has 6,770 lines and `executor.rs` has 3,748. The criterion is which owner must change when an index, predicate implementation, result consumer or cost rule is replaced.

| Owner | Move here and call from production |
| --- | --- |
| `prepared.rs`, `query_execution/{request,domain}.rs` | Frozen query and snapshot, prepared leaf table, checked request context, domain cursor and request-local instrumentation. |
| `query_predicate/{mod,structured,text,relation,scan}.rs` | Predicate binding and semantic properties, exact matcher, source projection, evidence and three-valued evaluation. Supply a single scan program to both base and segment readers. |
| `query_access/{native,posting,metadata,text,relations}.rs` | Executable native offers and storage-specific implementation, including the native segment provider now at the top of `executor.rs`. Keep posting, column, gram and relative-path range kernels with their respective adapters. |
| `query_plan/{selection,boolean,ordering,operations}.rs` | Put legal AND/OR ordering, FILTER/FTS cost comparison, fused choices, traversal selection and early-stop eligibility here. Replace the identity-order functions in `boolean.rs` and unused ordering structures with the policy used by the runner. |
| `query_execution/{source_scan,segment_scan,relations}.rs` | File/range jobs, source sessions, cancellation, residual evaluation and relation execution using the shared prepared program. Give modules explicit imports and narrow inputs instead of depending on a coordinator-wide `use super::*` namespace. |
| `query_results/{ordered,delivery,cache,materialize,ranking}.rs` | Move lifecycle ordered pages/streams, winner admission, cache replay and row finalization out of `runner.rs`. Use the same owned producer for synchronous pages and borrowed stream events. |
| `query_diagnostics/trace.rs` | Render estimates and request-scoped measured observations. Own diagnostic aggregation and projection; retain selection and native storage decisions in their respective owners. |
| `run.rs`, `query_execution/runner.rs`, `executor.rs` | Preserve existing caller paths. The coordinator prepares, selects, executes domains and finalizes. Remove superseded kernels and test-only alternate pipelines after migrating their callers. |

Keep the cache, sort and relation fast paths when exactness, evidence, order, limits and snapshot visibility permit them. For each move, list the old symbol, new owner and production caller. A new module containing wrappers while the original decision still lives in the runner is incomplete.

## 8. Commit order, validation and handoff

1. Add the failing native count/FTS, same-span, empty-domain and dispatch regressions. Land the minimal false-positive fix first. Record independently which tests failed at baseline and why.
2. Introduce stable term/domain identity, ownership-aware membership and the prepared request. Migrate base and segment provider application and matcher dispatch. Run the three-configuration public conformance fixture at this point.
3. Convert proof ownership, streaming and page accounting. Run allocation/identity, cache-eviction, bounded-memory and cancellation tests.
4. Migrate planning and specialized algorithms to the named owners. Remove test-only engine selection. Integrate actual collection/operation observations and the concurrency tests. Keep move-only commits separate from semantic corrections where practical.
5. Run the final core validation and benchmark once on the integrated code. Push one reviewable branch and record the implementation SHA separately from the final evidence commit.

Run from the repository root with its toolchain and lockfile.

```sh
cargo fmt -p objekte-search-core -- --check
cargo check --locked -p objekte-search-core --all-targets
cargo test --locked -p objekte-search-core
cargo build --release --locked -p objekte-search-core --example bench_musenalm
```

Use the same Musenalm corpus, schema, worker count and cache settings as the reviewed run. Set `MUSENALM_ROOT` to the local corpus and `RESULT_DIR` to a fresh directory for the measured revision. Run the release binary built in that worktree.

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

Compare against the existing reviewed-code and original-master samples in `docs/ai/generated/query-execution-refactoring/repairs/final-f0df3ba6/`. Reuse them when comparable. If the environment changed, run the affected baseline and final cases back-to-back. Separate normal query timings from analysis. Compare medians, ordered result digests and ordinary work counters. Corrected results are expected to differ on the new false-positive fixture; establish correctness from the reference evaluator. Investigate any existing suite digest change individually. For a repeated slowdown exceeding **both 5% and 0.2 ms**, rerun only the affected case with alternating revision order, pinning the same settings. Include Q141, its physical application counts and the explanation for any extra work. A timing change by itself does not establish its cause.

**Completion report.** Include the reviewed and delivered SHAs, test names mapped to sections 1–7, the actual native and alternate operation sequences for both orders, counts with evidence on/off, an example spanning two domains, trace isolation results, ownership/budget measurements and the old/new module ownership table. Give measured candidates and final matches separately. Identify any unfinished requirement explicitly and leave the task open until its acceptance test passes on the production route.

## Implementation report

- Task: `core-query-execution-corrections`
- Completion date: 18 November 2026
- Base commit: `74e569f505b8059aba1befef517c0c58e1e87bc0`
- Reviewed code commit: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`
- Delivered implementation commit: `4baa7af067f74f5ffb5772ef641ff0994fc4173f`
- Integration branch: `integration/core-query-execution-corrections`
- Integration worktree:
  `/home/simon/source/objekte-integration-core-query-execution-corrections`

### Summary

The correction makes FILTER/FTS completion depend on proof of every required
predicate, including segmented FTS. The former FILTER-only shortcut admitted
record 4 with evidence disabled; the public regression now returns record 3
through search, zero-limit exact count and ordered streaming.

Native segmented structured operators now select FILTER/FTS order from the
same request-local provider costs and apply the second operation to the first
operation's effective survivor set. Test configuration can select native,
independent-provider and source-verification configurations without restoring
a public test-only engine switch.

Prepared Boolean shapes retain occurrence IDs independently of source spans.
Residual and operation lookup use prepared semantic occurrences rather than
`(span,path)`, while text operations receive the prepared FTS ID. Segment
domains now carry the owning repository instance at full width.

Request observation recording requires a request-local capture token before it
constructs strings or term vectors. Phase markers and post-capture
normalization distinguish ordered-page and exact-count work, including Q141.
Plan candidate counts remain separate from final match counts.

Delivery accounting treats moved page owners as live rather than new projected
copies and checks the event-size limit independently from retained-buffer
accounting. Ranking winner-byte policy now has its production owner in
`query_results/ranking.rs`.

### Commits

- `4d2ac6623abcbed3237d7b56e6ce8f51e5c53a21` — require complete segmented
  predicate proof and carry repository instance into segment domains.
- `71636c4fe3343b0b1dea67c0ade31e96b68fca4b` — prepared occurrence/domain
  corrections, segmented provider ordering, effective-input restriction,
  source/trace isolation, delivery accounting, tests and core formatting.
- `59089254ac07c195ad52680aa82685cb93ed8b1f` — label exact-count observations.
- `4baa7af067f74f5ffb5772ef641ff0994fc4173f` — distinguish follow-up count
  observations after an ordered-page pass.

### Tests mapped to requirements

| Requirement | Production test |
| --- | --- |
| 1, initial Gate A | `gate_a_public_search_selects_either_provider_in_either_order` |
| 2, proof composition | Gate A evidence-disabled search/count/stream; `same_span_occurrences_keep_distinct_plans_and_exact_semantics` unindexed-text case |
| 3, occurrence/domain identity | `same_span_occurrences_keep_distinct_plans_and_exact_semantics`, `repeated_equal_terms_keep_distinct_request_local_ids`, `every_domain_component_participates_in_identity` |
| 4, interchangeable access | Gate A native/independent/scan-only configurations; `restriction_never_widens_the_original_domain_or_supplied_input`; cost crossover and deterministic tie tests |
| 5, ownership/accounting | `unrestricted_empty_backing_is_canonical_empty_for_every_representation`, exact shared-owner tests, delivery replacement and budget tests |
| 6, trace correctness | Gate A ANALYZE observations, `observation_capture_is_request_thread_local`, Q030/Q141 generated plans |
| 7, module ownership | selection/ranking/provider/predicate/result modules referenced by the table below |

At completion:

- `cargo fmt -p objekte-search-core -- --check` passed.
- `cargo check --locked -p objekte-search-core --all-targets` passed.
- `cargo test --locked -p objekte-search-core` passed 569 library tests, all
  integration tests, and the documentation compile-fail test. Sixteen existing
  manual/profiling tests remained ignored.
- `cargo build --release --locked -p objekte-search-core --example bench_musenalm`
  passed.

### Operation sequences

Gate A, text first:

```text
FTS title "rare": input {0..7}, candidates {0,3,4}
year = 1800:      input {0,3,4}, candidates {3,4}
exact verification: final {3}
```

Gate A, structured first:

```text
year = 1800:      input {0..7}, candidates {1,3,4,6}
FTS title "rare": input {1,3,4,6}, candidates {3,4}
exact verification: final {3}
```

Evidence off returns exactly one record with ID `3`; evidence on returns IDs
`title` and `year`. Count-only execution uses the same final-membership rule.

### EXPLAIN ANALYZE

Q030 now records 2,251 candidates separately from 1,264 final matches.

Q141 records ordered-page work as:

```text
year >= 1800: input 4928, candidate/proven 3821
year < 1850:  input 3821, candidate/proven 3308
FTS Almanach: input 4928, candidate 1627, proven 0
```

The follow-up exact-count pass is labeled separately:

```text
FTS Almanach: input 4928, candidate 1627, proven 0
year >= 1800: input 1627, candidate/proven 1151
year < 1850:  input 1151, candidate/proven 982
```

The repeated physical work is retained and labeled rather than inferred.
No physical applications were removed by this final observation-labeling
change.

### Module ownership

| Responsibility | Production owner |
| --- | --- |
| FILTER/FTS order preference, native cost split | `query_plan/selection.rs` |
| Legal Boolean metadata | `query_plan/boolean.rs` |
| Relation fallback ordering | `query_plan/operations.rs` |
| Prepared predicates, residuals and source projections | `query_predicate/{mod,scan}.rs` |
| Base and segment native structured/text offers | `query_access/{native,text,registry}.rs` |
| Source, segment and relation scanning | `query_execution/{source_scan,segment_scan,relations}.rs` |
| Ordered pages, cache replay and materialization | `query_results/{ordered,cache,materialize}.rs` |
| Ranking and winner byte accounting | `query_results/ranking.rs` |
| Provisional page and replacement delivery | `query_results/delivery.rs` |
| Observation capture and phases | `query_execution/request.rs` |
| Trace attachment/rendering | `query_diagnostics/trace.rs` |

The coordinator retains request sequencing and candidate composition. Cost,
storage, predicate, ranking and delivery decisions have explicit module owners
and production callers.

### Performance

Full raw evidence is in
`docs/ai/generated/core-query-execution-corrections/`; the summary is
`docs/ai/generated/core-query-execution-corrections/REPORT.md`.

The delivered five-sample ascii-off medians matched the reviewed baseline for
existing results and work counters. Q164 crossed both timing thresholds once;
alternating 20-repetition checks did not reproduce a consistent slowdown:
delivered first was 6.123 ms versus base 6.606 ms (-7.3%), and delivered
second was 6.434 ms versus interleaved base 6.606 ms (-2.6%).

Q030 crossed both thresholds once; alternating 20-repetition medians were
2.820 ms and 2.997 ms versus base 2.800 ms and 2.922 ms (+0.7% and +2.6%).
Candidate, parse, residue and result counters were unchanged.

### Cleanup

No subordinate worktree was created. The integration branch and worktree are
retained for the user's merge decision.
