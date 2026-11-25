# Objekte: master library review and improvement plan

Reviewed revision: `81788174898c27b2c2c367319bad20ecee450562` on `master`, 7 November 2026.

Scope: `crates/core`, the library documentation, and selected unit and integration tests. All 42 production Rust files were read, including the internal inference builder: 28,369 lines in those files, including their small inline test sections. The CLI and TUI are outside this review's implementation scope.

This is a source review and an implementation work order. No repository code was changed. Cargo was unavailable in this review environment, so the identified defects are source-traced findings, not claims of executed regression tests. Performance opportunities below identify avoidable work and scaling limits; they are not measured speedup estimates. The first implementation steps must reproduce the defects and establish measurements on the user's hardware.

## 0. Mandatory agent and worktree contract

**The deliverable is one completed, validated integration branch that the user can merge into `master` personally. Agents must not perform that final merge.**

The user's original checkout must remain on `master` throughout execution. Its checked-out branch, `master` commit, index, and working files must remain unchanged by this work. Do not implement, format, build, benchmark, create reports, resolve conflicts, or integrate changes in that checkout. Do not switch it to another branch, pull into it, reset it, clean it, or stash its existing changes. Read-only inspection and Git worktree registration are permitted; creating task/integration refs does not authorize moving `master`.

Use a coordinator agent, a dedicated integrator subagent, and implementation subagents for independently assignable work. Each implementation subagent receives its own task branch and worktree. The integrator has a separate integration branch and integration worktree. All worktrees must be direct children of `/home/simon/source`, never `/tmp` and never nested inside the original checkout. Any coordinator code changes require a separately assigned task worktree as well.

The integrator combines task branches, resolves integration issues, runs combined validation, and prepares the final handoff. The coordinator supervises dependencies and file ownership; it does not implement or merge in the original checkout. Independent work runs concurrently where useful. Changes to the same execution subsystem are sequenced where concurrent editing would create unnecessary conflicts.

Section 8 defines the required setup, task allocation, integration procedure, and final acceptance checks. These requirements apply to every phase, including tests, benchmarks, documentation, and integration. This document authorizes planning the workflow; receiving it as an implementation assignment starts the workflow. Correcting this document has not created branches or begun implementation.

## 1. Assessment and recommended order

The library has a useful foundation: immutable published generations, typed postings with explicit coverage, a separate reference execution route, source-reader extensions, diagnosed raw fallback, bounded result retention, and explicit snapshot persistence. Keep that foundation.

The largest opportunities are concentrated in four areas:

1. **Planning does execution work repeatedly.** Inferred-index estimates construct candidate bitmaps; temporal estimates also construct result bitmaps. Boolean ordering can repeat this work many times. Some diagnostic planning reads and parses source records.
2. **Ordering repeats expensive work or prevents concurrency.** Inferred orders are reconstructed for every request. Computed sorting sets the source batch size to one file. Related sort evaluation holds a cache mutex across target loading and decoding.
3. **Resource control is inconsistent across execution routes.** There are several independent budgets, late checks, hard-coded limits, and cancellation gaps during relation planning. Whole-file source buffers and whole-collection snapshot buffers remain substantial scaling limits.
4. **Execution responsibilities are duplicated.** Planning, evidence handling, ranking, payload loading, accounting, and streaming appear in several routes. This increases both maintenance cost and the probability that a feature behaves differently between ordinary, cached, streamed, and diagnostic execution.

There are also small, concrete correctness defects worth fixing before performance refactoring.

| Order | Work | Why it comes here |
| --- | --- | --- |
| 1 | Regression fixes: prepared cache, JSON acceptance, composite status order, dictionary accounting, numeric invariants | Small scope; protects correctness before changing execution |
| 2 | Cheap planning and reusable plan decisions | Removes repeated work from many query types |
| 3 | Inferred ordering, computed-sort scheduling, bitmap selection/count | Directly improves indexed queries, pagination, and expensive sorts |
| 4 | Build-time inference, diagnostics, and relation validation | Targets construction costs without reducing index coverage |
| 5 | Resource controls, large-file framing, snapshot buffering | Makes large workloads predictable and exposes limits clearly |
| 6 | Module boundaries, public API, and documentation | Consolidates the resulting design instead of reorganizing code that will immediately change again |

Do not start with a complete rewrite, a different index structure, a new query language, or a wholesale replacement of `Value`. There is substantial avoidable work within the existing design.

## 2. Existing improvements that must remain

These are implemented in the reviewed revision and must not be assigned again as missing work:

- Ordinary repository opening and rebuilding do not automatically save a snapshot. Keep persistence behind explicit save calls or an explicitly requested write policy. Do not add saving to `Drop`.
- Snapshot serialization already uses a single `serialize_into` pass and exposes serialization, checksum, write, and synchronization measurements. Do not reintroduce a preliminary serialization-size traversal.
- Inference already interns field names, reuses traversal path storage, uses borrowed lookup for repeated short strings, and avoids creating ordinary inferred posting maps for manual index paths and explicit opt-outs.
- Short strings have a separate ordered dictionary with a default inclusive limit of **64 decoded UTF-8 bytes**, sufficient for ordinary textual UUIDs. Partial short-string coverage is tracked. Preserve that distinction from complete scalar coverage.
- Complete supported inferred scalar predicates can already avoid source reads. Inferred scalar ordering also already exists; its issue is repeated construction cost, not missing functionality.
- Forward relation semijoins, source-dependent target prefetching, bounded streaming, parameter binding, temporal queries, regex filtering, and posting-value inspection already exist.
- Whole-file replacement and incremental publication are explicitly future work. Their absence is not a regression to repair in this plan.

## 3. Correctness findings and focused fixes

### C1. Prepared-cache replacement does not maintain its accounting

**Evidence:** [`PreparedCache::insert` and `prepare`][prepared]. Preparation releases the mutex while binding. Two concurrent requests can miss the same key, bind independently, and both insert. `insert` adds the new charge and another queue entry without removing the old charge or queue entry. The map then replaces the previous value.

Consequences include incorrect retained-byte accounting, duplicate eviction entries, and eviction of a newer value through an older queue entry. The verified-result cache already handles replacement more carefully.

**Implementation:** make same-key insertion idempotent or implement explicit replacement accounting. Keep expensive binding outside the mutex. Compute the entry's charge before acquiring the insertion lock. Consider shared immutable prepared values separately; do not make the correctness patch depend on a new cache framework.

**Regression cases:** repeated insertion of the same key; different-size replacement; eviction after replacement; and two barrier-coordinated concurrent misses. Assert both logical entries and accounting. Existing `prepared.test.rs` tests ordinary LRU eviction but not replacement.

### C2. Fresh-build JSON parsing and query parsing disagree

**Evidence:** [`ScalarWalker`][scalar-parser] accepts `u8::is_ascii_whitespace`, which includes form feed (`0x0C`), as confirmed by the [Rust API documentation](https://doc.rust-lang.org/std/primitive.u8.html#method.is_ascii_whitespace). JSON permits space, tab, CR, and LF. The fresh decoded-scalar builder can therefore accept input such as a leading form feed followed by an object, while the later serde-based query path rejects it. The builder already has a `depth > 128` guard; its accepted nesting boundary should also be tested against later reads.

**Implementation:** use precisely the JSON whitespace set. Preserve the existing depth protection and align build/query boundary behavior where necessary. Audit token boundaries and string handling with differential tests. Preserve the current one-pass fresh-build extraction; a second complete parse for every record is not the desired fix.

**Regression cases:** every ASCII control byte at document and token boundaries; nesting at and around the chosen limit; malformed numbers; Unicode escapes and surrogate pairs; trailing commas; all root types. Preserve the intentional duplicate-name rule: structured field access uses the final occurrence, whereas unscoped scalar FTS sees every occurrence. Compare production reader entry points, not just two helpers sharing the same parser.

### C3. File-status refresh leaves composite sort orders stale

**Evidence:** [`SortCollection::refresh_file_status`][status-refresh] updates the status column and its ascending/descending orders, but does not rebuild composite orders. [`build_fresh`][final-publication] performs this refresh after collections have been built and shared-file diagnostics have settled.

A file can validate in one collection and fail in another. Its final shared file status can then differ from the status used to build the first collection's composite order. That can produce an incorrect composite order; snapshot restoration's order validation can also expose the inconsistency.

**Implementation:** publish all orders from final file status. Either postpone dependent order construction until status is final or refresh every affected composite. Prefer a clearly defined finalization stage over several ad hoc refreshes.

**Regression cases:** two collections sharing at least two files, one schema rejecting one shared file, and a composite order containing `@file.status`. Choose IDs and secondary keys that reveal an inversion. Test both collection declaration orders, both directions where supported, reference parity, and explicit save/restore.

### C4. Repeated strings can incorrectly exhaust the distinct-byte summary

**Evidence:** [`FieldState::add_scalar`][distinct-bytes] checks `distinct_nonblank_bytes + value.len()` even when the incoming string is already present. With a three-byte limit, observing `"abc"` twice can drop a summary whose distinct text still occupies three bytes.

**Implementation:** charge new distinct text only when inserting a new value. Preserve separate per-value length and distinct-count limits. Check the merge path against the same invariant.

**Regression cases:** exact byte boundary, repeated values at that boundary, multibyte UTF-8, one new value crossing the limit, and equivalent results across batch partitions. The current test verifies the counter under default limits but does not test admission at the limit.

This is an inference-summary/admission defect. It does not establish that ordinary query membership is wrong; exact fallback is a separate safeguard.

### C5. Public numeric construction can violate equality and hashing

**Evidence:** [`Number` ordering and hashing][number-order]. `Number::float` rejects non-finite values, but the public `Number::Float` variant and derived deserialization can bypass that constructor. In `compare_int_float`, NaN falls through the integer conversion and fractional comparisons and can compare equal to integer zero. Hashing handles that NaN differently. Comparisons involving `ExactDecimal` assume finite floating-point input as well.

**Implementation:** choose and document a complete invariant for public construction and deserialization. Prefer validated construction; if the public enum must remain compatible, make all reachable values obey the required ordering/equality/hash laws and reject unsupported values before indexing. Preserve the established semantics of finite integers, floats, and exact decimals.

**Regression cases:** direct variants, deserialization, all non-finite float classes, signed zero, integer boundaries, and cross-representation equal values. Assert `Eq`/`Hash` consistency and ordering transitivity. This primarily concerns custom readers, extension authors, and binary data boundaries; ordinary valid JSON is not a NaN input route.

### C6. Path and binding parsing need one explicit contract

**Evidence:** [`FieldPath`][field-path], [`QueryPath`][query-path], [`Query::from_workload`][workload-path], and [`sort_binding::expression`][sort-expression] use overlapping representations and parsing rules. Examples include a trailing dot accepted by one parser, FTS binding extraction using `rsplit_once(':')` inside a quoted property such as `["a:b"]`, and a programmatic sort string such as `x:lower` being accepted as a bare path before the binding parser is attempted.

**Implementation:** distinguish a schema field address from an occurrence-preserving query path and from a path-plus-binding expression. Share tokenization and canonical rendering where their grammar is the same. Make the typed sort-expression constructor the unambiguous programmatic route. Define compatibility for existing string constructors rather than silently changing how colon-containing property names are interpreted.

**Regression cases:** quoted colons, spaces, operators, quotes, brackets, leading `@`, empty names if supported, root values, numeric object properties, and numeric/wildcard array selectors. Test parse/render round trips and consistency across FILTER, selected FTS, sorting, and schema registration.

## 4. Query performance work

### Q1. Separate estimates, physical plans, and execution

**Evidence:** [`enumerate_paths`][planner] calls `inference.candidates(...)` to obtain a cardinality. That constructs a candidate bitmap across the collection. [`estimate_expr` and Boolean planning][boolean-planner] repeat enumeration while selecting conjuncts and from the OR sort comparator. [`PostingIndex::estimate_exact`][temporal-estimate] obtains temporal estimates by running the temporal posting operation. Residual ordering invokes estimates again. A stored-order attempt can also construct a plan and then return `None`, after which the general route plans again.

This is the strongest broad performance opportunity identified by source inspection.

**Implementation:**

1. Introduce a cheap estimate operation for inferred paths, using maintained counts, dictionary statistics, coverage, and conservative bounds. It must not construct result bitmaps.
2. Compute invariant per-term estimates once. Recalculate only the input-dependent component as the candidate set narrows. Cache sort scores before sorting OR children.
3. Execute the selected access path once. Reuse the resulting plan/candidates when selecting ordered traversal versus top-k.
4. Use bounded B-tree ranges for compatible temporal and scalar comparisons. Preserve null, absence, array, partial coverage, and complement semantics.
5. Build detailed alternative-path strings only when diagnostics require them, while retaining compact decisions needed by execution.

**Acceptance:** reference-equivalent results on the existing interaction suite; counters for estimation calls, dictionary keys visited, candidate bitmap constructions, and reused plans. An AND of 1, 4, and 16 inferred terms should no longer repeatedly execute the candidate generator merely to order those terms. Do not require perfectly accurate cardinalities from the estimator.

### Q2. Make non-analyzing EXPLAIN an estimate-only operation

**Evidence:** [`run::explain`][explain] calls the executable planner and relation FTS witness construction. Those operations can open and parse target records. It also constructs inferred ordering state when deciding the reported sort strategy.

**Implementation:** define a clear separation between an estimated plan and `explain_analyze`. Use the same physical decision representation as ordinary execution, with unknown or estimated cardinalities clearly marked. If existing users rely on executable diagnostics, preserve that capability through an explicitly named mode and document the compatibility change.

**Acceptance:** instrumented readers record zero source reads and zero parses for estimate-only EXPLAIN, including relations and related FTS. Analyze reports actual work. Estimate-only planning does not build an inferred sort column merely to report that it is eligible.

### Q3. Reuse inferred ordering across pages

**Evidence:** [`inferred_order_column`][inferred-order] constructs a temporary `SortColumn` for each eligible query. [`SortColumn::from_inferred_posting`][inferred-column] allocates a key for every record, constructs order vectors, generates ICU keys for strings, and sorts the string order. The search entry point bypasses the verified membership cache for this route. Avoiding source reads is valuable, but repeated CPU and allocation work remains.

**Implementation:** compare two bounded options before choosing:

- A generation-scoped lazy order cache keyed by field, locale, collation version, placement options, and direction.
- A dictionary cursor with reusable key/group ordering where it can preserve the same semantics without per-record key duplication.

Numeric/Boolean dictionary order and locale-aware string order require different handling. UTF-8 B-tree order is not a replacement for ICU collation. Reuse collation keys per distinct string where practical. Retain stable collection/ID tie breaking and missing/null/unavailable placement.

**Acceptance:** repeated pages on an unchanged generation do not rebuild the same complete ordering. Report first-use cost separately from reuse. Exercise UUID-like high cardinality, repeated strings, non-ASCII strings, missing/null records, both directions, count modes, and cache eviction. Preserve fallback for incomplete inferred coverage.

### Q4. Restore useful concurrency for computed sorts

**Evidence:** [`search_impl` scheduling][computed-scheduling] selects `base_chunk_size = 1` whenever a computed sort exists. This makes the per-file parallel loop effectively sequential. Separately, [`BoundSort::keys`][related-sort-lock] holds the related-target cache mutex across traversal and possible target reads/parsing. The mutex becomes a concurrency obstacle once scheduling is improved; it is not evidence of current worker contention when only one file is scheduled.

**Implementation:**

1. Reserve memory for a bounded group of workers and local top-k keys instead of disabling parallelism for every computed sort.
2. Make key and membership processing independent of full payload retention. Materialize final page payloads after ranking where this lowers memory; retain or reuse source data only when measured beneficial.
3. Move target I/O and parsing outside a shared global lock. Evaluate immutable prefetch, per-target load coordination, or bounded worker-local storage. A cache hit should not require holding a lock throughout a complete relation traversal.
4. Use projections for computed keys where reader semantics allow it; the current route broadens extraction to a full record.
5. Share target-cache mechanisms only where their representation, coverage, and lifecycle match. Do not force filter prefetch and sort traversal into an unsuitable common abstraction.

**Acceptance:** source-independent computed sorts use multiple workers under an adequate budget. Related sorts show target reuse and bounded memory without a global I/O critical section. Compare workers 1/2/4/8, many small files versus one large JSONL source, and high versus low target reuse. Preserve FIRST/LAST occurrence order, dangling-target truth, cancellation, and deterministic ties.

### Q5. Add dedicated count and exact-membership execution

**Evidence:** [`exact_members`][exact-selection] collects a `Vec<CachedMember>` with an effectively unlimited tracker and then converts it into a bitmap. It does not need full `SearchHit` payloads, but still uses an intermediate structure per match. Ordinary zero-limit/count requests can also retain unnecessary ordering work; computed-key evaluation happens before the page admission decision.

**Implementation:** add an internal result collector for count and bitmap membership. Return exact candidate cardinality directly when the plan proves membership. For residual work, accumulate a count or bitmap without ranking, evidence strings, or cache-member vectors unless explicitly needed. Validate supplied expressions even if sorting is irrelevant to a count; do not silently accept an invalid query.

Expose a convenient count operation if it makes the public API clearer, but avoid adding another independent executor. Reuse this collector for `select_exact` and scoped posting-value counts.

**Acceptance:** index-proven count has no per-hit ranking and no source reads; residual count has no payload retention; exact selection writes directly into bitmap membership. Test zero matches, zero limit with nonzero offset, large matches, relations, fallback records, and parameterized requests.

### Q6. Move repeated evaluator work to binding

**Evidence:** [`contains_pattern`][wildcard] splits and allocates the same wildcard pattern on each scalar comparison. It is also called while scanning inferred dictionaries. Field comparison builds scoped-value vectors and result vectors; mixed relation/local FTS clones a bound FTS structure per field per record. FTS evidence is often collected even when the caller only needs membership.

**Implementation:** prepare wildcard pieces once, including the plain substring fast path. Add membership-only evaluators that can stop at the first decisive result and avoid evidence allocation. Use small inline storage or visitors for common scalar paths where profiling supports it. Pass borrowed single-field FTS views instead of cloning complete bindings.

Do not merge all walkers mechanically. Positional pairing, implicit array traversal, unavailable truth, and duplicate-member text extraction have different semantics. Keep the reference evaluator independently understandable.

**Acceptance:** allocation measurements on scalar filters, wildcard filters, arrays, FTS without evidence, and mixed local/relation FTS. Preserve `%` and escaped-percent semantics, empty-array quantification, and complete evidence when requested.

## 5. Index-construction work

### B1. Improve inference retention and stop unnecessary work

**Evidence:** [`inference_build.rs`][inference-build] maintains several per-path summaries and posting structures. Batch contributions are merged into coordinator state. Local retained-byte estimates can charge repeated metadata across batches. A collection-level incomplete result discards inferred indexes, while already scheduled or later independent worker work can still construct contributions that will not be published. Manual/blocked paths avoid posting maps but can still retain distinct-string summaries.

Current defaults are 16 MiB per batch, 512 MiB coordinator allowance, and 768 MiB repository allowance. These are estimates for inference state, not a process RSS ceiling. Increasing limits is not a substitute for reducing retained work.

**Implementation:**

1. Add phase and allocation measurements for observation, batch state, merge, and final admission. Compare equivalent published coverage.
2. Separate retained unique state from cumulative processing charges. Avoid counting repeated path/dictionary overhead as if all batch copies remain live after merge.
3. Bound distinct summaries for manually indexed or excluded paths according to their reporting purpose. Do not retain an effectively unbounded high-cardinality summary for an index that will not be constructed.
4. Let workers stop inference work promptly when a coordinator can prove the collection's inference result cannot be published, while continuing required source parsing and ordinary indexes. Ensure the stop mechanism cannot make admission depend on worker timing.
5. Consider field-specific discard instead of collection-wide discard only as a versioned admission-policy change with explicit completeness semantics. Do not present reduced coverage as a speed improvement.

**Acceptance:** identical required semantics and deterministic policy outcomes across worker counts and batch partitions; explicit reporting of admitted/dropped paths and reasons; lower retained allocation or lower inference CPU on the same workload. Preserve the zero-inference path when disabled.

### B2. Remove duplicate relation-validation reconstruction

**Evidence:** [`RelationContribution::resolve` and `RelationIndex::validate`][relation-validation] reconstruct a reverse dictionary from every edge to validate it. Repository finalization calls `validate_indexes` again, repeating the reconstruction for the freshly built relations.

**Implementation:** retain one complete final validation at the publication boundary or replace a redundant intermediate reconstruction with local construction invariants. Keep full validation of restored/untrusted snapshot state. Do not remove the independent relation oracle from tests.

**Acceptance:** one reverse reconstruction per fresh publication at most; unchanged detection of broken offsets, wrong target slots, unresolved IDs, and forward/reverse inconsistency. Measure construction time and allocation with large fanout and repeated target IDs.

Only after measuring this change, consider compact inline occurrence positions or fewer target-ID copies. That is a secondary representation optimization.

### B3. Make malformed-file diagnostics linear

**Evidence:** [`file_coordinates`][file-coordinates] scans the entire source prefix for every malformed record. A large JSONL source containing many malformed frames can therefore spend quadratic work counting preceding newlines. Limiting stored diagnostics does not prevent this computation before the diagnostic is passed downstream.

**Implementation:** carry line state through framing/extraction or build a compact reusable line-position structure. Select the approach that preserves bounded memory and existing custom-reader coordinates. Skip coordinate work for diagnostics that will definitively be discarded only if the API's reporting behavior permits it.

**Acceptance:** correct file-relative byte/line/column coordinates with LF and CRLF, multiline records, and errors at frame boundaries. A doubling test with similarly distributed malformed frames should show approximately linear coordinate work.

### B4. Investigate shared-source reuse and schema compilation only where relevant

The build coordinator currently schedules work by collection, so shared physical files can be framed and parsed separately for multiple collections. Some documentation describes more sharing than this implementation provides. Reuse raw extraction only when source registration, reader version, and text-derivation semantics are compatible; schema conversion and validation remain collection-specific.

Also, schema registration can rebuild conversion validation after successive registrations. A compile/finalize step may help large schemas, but this is startup configuration work and is lower priority than query planning and record processing.

These are workload-dependent experiments. Benchmark shared collections and large schemas first. Do not complicate the common single-collection build for an unmeasured saving.

## 6. Resource control and large-data limits

### R1. Use one execution context for cancellation and resource decisions

**Evidence:** relation traversal, relation semijoins, target caches, inferred ordering, computed sorting, and streaming each enforce separate limits. Some query-wide checks occur only after a large unit of work completes. The relation planner does source work before returning to the outer stream cancellation check. Several paths use source length multiplied by a constant as a proxy for retained logical memory; extension readers can expand input differently.

**Implementation:** introduce a small execution context carrying a frozen clock, cancellation, explicit limits, and shared accounting. Thread it through expensive planning/execution operations. Check cancellation and budgets at bounded intervals before large allocations or reads. Keep the no-cancellation common path inexpensive. Replace the prepared cache's debug-string size estimate with an explicit retention estimate, including compiled matcher ownership; consider shared immutable cache values to avoid cloning complete bound trees while holding its mutex.

Use typed errors for cancellation, resource exhaustion, source changes, invalid queries, and extension failures. The current stream boundary recognizes some resource errors through string prefixes or `"budget exceeded"`; error text should not determine control flow. Keep CLI-specific recovery advice in the CLI rather than library error strings.

Clarify which limits bound retained result state, source buffers, and estimated extension allocations. Do not claim a process RSS ceiling that the implementation cannot enforce.

**Acceptance:** cancellation during relation planning, target verification, inferred-order construction, and large computed sorts; resource rejection before an unbounded allocation; consistent error classes across ordinary and streamed execution. Use small deterministic limits in tests. Include extension readers with large logical expansion.

### R2. Add an optional bounded framing route for large files

**Evidence:** [`frame_file_for_build`][full-file] reads an entire file into an `Arc<Vec<u8>>` and returns all record ranges. Bounded downstream jobs do not bound that source allocation. Query scanning is also grouped by physical file, which limits parallelism for a single large multi-record file.

**Implementation:** add an optional streaming or bounded-range framing capability for formats such as JSONL. Keep the existing whole-buffer interface for readers that require it. Carry absolute ranges, malformed-record diagnostics, record identity, and file ordering through bounded batches. Evaluate safe record-batch query parallelism separately from build framing.

**Acceptance:** large JSONL indexing has bounded additional source-buffer memory as file size grows; oversized individual records have an explicit policy; custom readers retain their existing behavior. Avoid new repeated filesystem metadata checks during fresh construction—the user has explicitly prioritized build speed over detecting concurrent source edits during that phase.

### R3. Remove the whole-collection snapshot buffering limit when scale requires it

**Evidence:** [`write_section_measured`][snapshot-write] serializes a section into a `Vec` before checking the 512 MiB section limit. The writer stores the inventory in one section and each collection in another section containing its index families. A sufficiently large collection therefore fails explicit persistence and can allocate a large temporary buffer before reporting the limit.

**Implementation:** first use existing save statistics to separate bytes written, serialization, checksum, write, and `sync_all`. More indexes legitimately mean more bytes; a slower save alone does not establish serialization inefficiency.

If large collections are a supported target, design versioned chunked sections or bounded serialization through the temporary file, with incremental checksumming and finalized headers. Preserve atomic replacement, explicit synchronization, compatibility rejection, and failure cleanup. Borrow inventory/configuration data where useful, but treat those smaller copies as measured follow-up work.

**Acceptance:** explicit save and restore of a collection larger than the current section limit; bounded temporary memory; corruption/truncation rejection; failed save preserves the previous snapshot. Opening, searching, and dropping a repository still produce no implicit snapshot writes. Do not remove checksum validation or synchronization to make the benchmark look faster.

### R4. Make freshness guarantees truthful and explicit

The source code and `docs/search-core.md` disagree: documentation promises checks of both the opened handle and named path before and after access. Ordinary range reads check the opened handle before reading; buffered reuse can perform no additional check.

Document the actual immutable-generation and read-consistency contract. Separate build-time behavior from later payload reads. If strict concurrent-mutation detection is required, offer and test an explicit policy instead of imposing repeated metadata operations on every fast path. Include same-size edits, renamed/replaced files, and buffered reads in contract tests; do not promise content-change detection from size/mtime alone.

### R5. Measure sparse multi-collection inventory overhead

`FileRecordIndex` uses file-indexed offsets sized against the global inventory for each collection. With many mostly disjoint collections, metadata can grow with collections × total files rather than collection memberships.

Add a many-collection measurement and consider a collection-local file map or sparse representation if this becomes material. Preserve efficient `FileId` lookup and snapshot validation. This is a targeted scale optimization, not a reason to replace the inventory model globally.

## 7. Organization and missing public operations

### O1. Split execution by responsibility after behavior changes

`run.rs` is 4,549 lines, `reference.rs` 2,572, `repository.rs` 1,626, and `executor.rs` 1,416. Size alone is not the defect; the repeated responsibilities are.

Recommended boundaries:

| Responsibility | Desired ownership |
| --- | --- |
| Query preparation | Binding, parameter constraints, prepared immutable state |
| Planning | Cheap estimates, physical alternatives, chosen access paths |
| Execution | Membership proofs, residual evaluation, candidate production |
| Ordering | Stored cursors, inferred orders, computed keys, top-k merging |
| Materialization | Grouped source reads and payload conversion |
| Delivery | Page/all-results scope, events, backpressure, cancellation |
| Measurement | Work accumulation and optional diagnostic rendering |
| Repository construction | Discovery, build scheduling, final validation, publication |

Extract repeated payload-loading and work-counter accumulation first. Keep specialized algorithms for stored-order traversal and computed top-k; unify their interfaces, not their inner loops. Keep a reference execution route that can independently detect optimized-path errors.

Move runtime `ReturnMode`, `SortField`, alphabet configuration, and counters out of a module that also owns fixed benchmark manifests. Retain compatibility re-exports initially. Move the remaining inline tests to separate test files in accordance with repository conventions.

Each structural commit must be behavior-preserving and independently reviewable. Do not combine a major module move with a semantic correction.

### O2. Add the public operations that support actual library users

| Operation | Present situation | Proposed change |
| --- | --- | --- |
| Inferred posting-value inspection | `posting_values` requires a registered schema field | Support eligible inferred dictionaries, returning explicit partial/unresolved coverage where relevant; never claim complete facets from partial short-string postings |
| Direct record retrieval by collection and ID | Inventory/ID lookup exists; normal callers largely compose lower-level access or a query | Add a small lookup/read facade with locator, original bytes, or logical payload and existing stale/unavailable semantics |
| Efficient exact count and selection | Expressible through queries; selection builds an intermediate member vector | Implement Q5 and expose a simple facade |
| Reusable prepared request | Internal cache only; hits clone bound trees under a mutex | Consider an opaque generation-bound prepared handle after preparation ownership is stable; distinguish query template preparation from execution-bound parameters and time macros |
| Consistent execution options | Cancellation is primarily exposed through streaming; resource limits are scattered | Provide R1's context/options through the public facade without duplicating methods for every combination |

Keyset pagination over sorted results is a useful later feature for very deep pages, but requires a generation-bound cursor and explicit collation/tie semantics. It is not required for this first improvement cycle. Posting-value cursors already exist and should not be confused with record-result cursors.

### O3. Correct documentation alongside the affected change

`docs/inference.md` still describes older memory defaults, omits current short-string behavior, and describes an older restricted JSON value model. `docs/search-core.md` describes automatic snapshot replacement and stronger freshness checks than the current ordinary opening/read paths provide. It also describes snapshot sections more finely than the current per-collection writer implements.

Make documentation describe current behavior, distinguish estimated limits from hard guarantees, and explain optional feature interactions. Prefer tests against exported defaults for documented numerical values. Keep benchmark examples separate from public library concepts. The CLI can remain a useful end-to-end test and benchmark driver, but library measurements must separate startup, save, query, and output costs.

## 8. Implementation phases and commit boundaries

### 8.1. Roles and ownership

| Role | Responsibility | Working location |
| --- | --- | --- |
| Coordinator agent | Verify the starting state, create/register worktrees, assign subagents, manage dependencies and write ownership, track evidence, and deliver the final result | Original checkout for read-only inspection only; all generated files and any implementation belong in assigned worktrees |
| Integrator subagent | Sole writer to the integration branch; review task changes, merge accepted commits, resolve integration issues, run combined checks, and prepare the handoff | Dedicated integration worktree |
| Implementation subagents | Reproduce findings, implement assigned changes, add focused tests, benchmark relevant changes, and commit completed work | One exclusive task branch and worktree per active assignment |
| Validation/review subagent | Independently review the combined diff and validate a fixed integration commit | Separate review worktree at the recorded commit; report findings to the integrator |

Use up to four implementation subagents concurrently when resources permit, leaving capacity for coordination and integration. Reduce concurrency for compiler/memory pressure or overlapping ownership. The integrator and reviewer are separate roles; do not have a reviewer examine an integration worktree while it is changing. If concurrency slots are limited, reuse completed implementation agents for review after releasing their previous assignments.

Only one agent may write a given worktree at a time. Subagents must not edit another agent's files through absolute paths, even though the worktrees are accessible in the same filesystem. Do not run global formatting from the original checkout or from another agent's worktree.

### 8.2. Initial setup: preserve the original checkout

Record the original checkout's absolute path, current branch, `master` SHA, and existing working-tree/index status before doing anything else. Verify that its current branch is `master`. If it is not, stop setup and report the mismatch; do not switch it automatically. Existing unrelated changes must be preserved and must not be included in task branches.

Create the integration branch from the captured committed `master` SHA. If it differs from the reviewed revision, verify findings against that starting revision before assigning fixes. Never infer that an uncommitted change is part of the baseline. Establish build/test and benchmark baselines inside the integration worktree before merging task work; record pre-existing failures separately.

Use this naming scheme for this execution, with a consistent additional numeric suffix if any path or branch already exists:

| Purpose | Branch | Worktree |
| --- | --- | --- |
| Final integration | `integration/library-improvements-20261107` | `/home/simon/source/objekte-library-integration-20261107` |
| Task example | `improvement/library-cache-correctness-20261107` | `/home/simon/source/objekte-library-cache-correctness-20261107` |
| Other tasks | `improvement/library-<task-name>-20261107` | `/home/simon/source/objekte-library-<task-name>-20261107` |
| Fixed-commit final review | Detached at the integration SHA; no deliverable branch | `/home/simon/source/objekte-library-validation-20261107` |

Use precise task names such as `json-parser-correctness`, `query-planning`, `inferred-ordering`, and `relation-validation`. Check `git worktree list --porcelain`, existing refs, and filesystem paths before creating worktrees. Do not reuse, reset, delete, or force-unlock an existing worktree whose ownership is unknown. Record the actual names if suffixes are needed.

The setup operations have this form; substitute the inspected paths and recorded commit, not an assumed repository location:

```sh
# Original checkout remains on master; these commands only register new worktrees/refs.
git -C "$original_checkout" worktree add -b "$integration_branch" "$integration_worktree" "$base_commit"

# Capture the current integration commit before dispatching each task.
task_base=$(git -C "$integration_worktree" rev-parse HEAD)
git -C "$integration_worktree" worktree add -b "$task_branch" "$task_worktree" "$task_base"
```

Every implementation, formatting, test, benchmark, and merge command must specify the intended worktree through its working directory, `git -C`, or an explicit manifest path as appropriate. Do not rely on the coordinator's current directory. Build artifacts, temporary benchmark datasets, cache/snapshot output, logs, and reports must be directed into task/integration worktrees or their explicitly configured per-worktree temporary directories. Source fixtures from the original checkout may only be read; copy them when a test or benchmark can modify them or write adjacent snapshots. Do not share writable build/cache directories between concurrent agents. Run comparative performance benchmarks sequentially so other agents' compilation does not distort results.

### 8.3. Dispatch assignments and dependency order

The phase table remains the implementation order. Each assignment below is executed by a subagent in a task worktree, not in `master`. A task may contain the separately reviewable commits specified in its technical section. Existing conditional experiments remain conditional; the coordinator records a measured deferral rather than silently omitting them.

| Phase | Commits | Completion condition |
| --- | --- | --- |
| 1: Correctness | Separate commits for C1, C2, C3, C4, C5; C6 after its compatibility decision | Reproductions fail before each fix and pass after it; existing behavior tests pass |
| 2: Planning | Cheap inferred/temporal estimates; reuse plan decisions; estimate-only EXPLAIN | No candidate construction for costing, no source reads for estimate-only diagnostics, measured planning improvement |
| 3: Query execution | Inferred-order reuse; count/bitmap collectors; computed-sort scheduling; evaluator preparation | Better repeated-page and sorted-query behavior with unchanged results and bounded memory |
| 4: Construction | Relation validation deduplication; linear diagnostic coordinates; inference accounting and stop behavior | Better build phases at equivalent coverage, deterministic outcomes across worker counts |
| 5: Scale controls | Execution context/typed errors; optional bounded framing; snapshot format work if justified | Explicit cancellation/resource behavior and supported large workloads |
| 6: Consolidation | Execution/materialization module extraction; public facades; documentation | Clear ownership, compatibility re-exports, documented guarantees |

Use the following division of work. File scopes refer to `crates/core/src` unless stated otherwise. Related tests and documentation are part of the assignment, but shared documentation files require a write reservation like code files.

| Assignment | Technical scope | Primary ownership | Dispatch condition |
| --- | --- | --- | --- |
| `cache-correctness` | C1 | `prepared.rs`, its tests | Initial independent task |
| `json-parser-correctness` | C2 | `json_scalar_text.rs`, reader regression tests | Initial independent task; reserve `source.rs` only if needed |
| `numeric-invariants` | C5 | `model.rs`, `decimal.rs`, numeric tests | Initial independent task |
| `inference-boundaries` | C4 | `inference_build.rs`, boundary tests | Initial independent task |
| `composite-status-order` | C3 | `sorting.rs`, minimal repository finalization changes, sort tests | May run when capacity permits; no other repository/sorting writer |
| `path-binding-contract` | C6 | `model.rs`, `query_path.rs`, `query.rs`, affected binding code/tests | After numeric and status fixes are integrated; reserve shared parser/binder files |
| `query-planning` | Q1, then Q2 in separate commits | `executor.rs`, relevant `posting.rs`/`inference.rs` operations, planning portions of `run.rs` | After Phase 1 is integrated; sole writer to execution/planning files |
| `inferred-ordering` | Q3 | `sorting.rs`, ordering portions of `run.rs`, tests | After planning changes are integrated |
| `exact-membership` | Q5 | Count/selection portions of `run.rs`, `posting_values.rs`, facade/tests | After inferred ordering; serialize edits to `run.rs` |
| `computed-sorting` | Q4, local scheduling and related-target changes in separate commits | `run.rs`, `sort_binding.rs`, target cache modules/tests | After count/selection changes; sole writer to the affected execution files |
| `prepared-evaluation` | Q6 | `reference.rs`, prepared matchers, `relation_query.rs`, evaluator tests | After C6 and planning; may run beside ordering only with explicitly disjoint file ownership |
| `relation-validation` | B2 | `relation.rs`, minimal `repository.rs` finalization changes, tests | After C3; may run beside query work if scopes remain disjoint |
| `diagnostic-coordinates` | B3 | Diagnostic coordinate/framing portions of `source.rs`, tests | After C2; may run beside query work |
| `inference-retention` | B1 | `inference_build.rs`, `inference.rs`, required `build.rs`/repository coordination, tests | After C4 and Q1; serialize repository changes with relation validation |
| `construction-experiments` | B4, R5 | Build/schema compilation and inventory experiments | After construction fixes; implement only justified experiments, each separately measured |
| `execution-context` | R1 | Execution options, typed errors, cancellation/accounting across query/relation/stream modules | After query tasks; broad ownership means no concurrent execution refactor |
| `bounded-framing` | R2 and relevant R4 code/contract work | `source.rs`, `source_access.rs`, `build.rs`, reader tests/docs | After parser/diagnostic fixes and shared context interfaces; reserve all affected build/source files |
| `snapshot-sections` | R3 | `storage.rs`, versioned persistence tests/docs | After resource interfaces settle and scale measurements justify it; coordinate any `MODEL_VERSION` change with integrator |
| `execution-organization` | O1 | Responsibility-based module extraction and compatibility re-exports | After behavior changes are integrated; exclusive ownership during module moves |
| `library-operations` | O2 | Public facade, inferred posting inspection, direct retrieval, consumer tests | After module boundaries stabilize; reserve relevant facade/index files |
| `documentation-consolidation` | O3 and remaining R4 contract documentation | Library documentation and final execution/results report | After APIs and behavior stabilize; verify against final code |

This table is a dispatch plan, not a requirement to run every row concurrently. In particular, do not launch several agents to independently rewrite `run.rs`. Create each dependent task from the then-current integration SHA, so it includes its prerequisites. Independent source/build work may overlap query work when ownership and dependencies allow it. Initial independent fixes can share the same base commit.

Each subagent assignment must contain: task identifier; absolute worktree path; branch; immutable base SHA; assigned findings; files it may modify; dependencies; required regressions/benchmarks; preserved semantics; and the prohibition on modifying `master` or integrating other tasks. If more files are needed, obtain an updated write reservation from the coordinator before editing them. Interface decisions crossing tasks must be communicated before implementation.

Each subagent returns its committed head SHA, commit list, changed-file list, reproduction/result summary, checks run, benchmark evidence, and known limitations. It must leave its task worktree clean. Uncommitted changes are not an integration handoff. Record findings that are already fixed or disproved instead of forcing an unnecessary patch.

### 8.4. Integration procedure: one writer, one deliverable branch

The integrator is the only agent allowed to advance the integration branch. Task subagents must not merge directly into it. Integrate completed assignments as soon as their dependencies and checks permit; do not postpone all conflict resolution until the end.

For each task:

1. Confirm the task's recorded base, exact head SHA, clean worktree, authorized file scope, and validation results. Review the diff, including accidental formatting and fixture changes.
2. Capture the integration head and ensure its worktree is clean. Freeze the accepted task head; additional task changes need a new handoff.
3. Merge that exact task commit into the integration branch from the integration worktree, preserving its implementation commits. Use ordinary non-fast-forward merges; do not copy files between worktrees or apply the same change through both merging and cherry-picking.
4. Resolve straightforward integration conflicts in the integration worktree. For semantic conflicts, assign the resolution to the responsible task agent in its own worktree, incorporating the recorded integration commit there, then review the new task head. Do not automatically choose all of either side and do not force-push/rewrite shared task history.
5. Run the affected combined tests and required checks. If integration exposes a defect, fix it in an explicit integration commit or a dedicated corrective task worktree. Keep the integration branch in a known validated state before dispatching dependent work. Record each accepted integration SHA and result.

Example merge location:

```sh
git -C "$integration_worktree" merge --no-ff --no-edit "$accepted_task_head"
```

If a merge is aborted, abort only the integrator's own in-progress merge in its cleanly established integration worktree. Never use a reset or cleanup in the original checkout to recover a task or integration failure. Master advancement, rebasing onto newly arrived master commits, and publishing remote branches are not implicit steps in this assignment; report external base changes and keep the recorded baseline unless the user directs otherwise. Never push `master`.

Task branches are intermediate implementation branches. The user must not need to merge or cherry-pick any of them. All accepted work, integration fixes, documentation, and validation results must be reachable from the single final integration branch.

### 8.5. Combined validation and independent review

At phase boundaries, the integrator runs the relevant combined tests from the integration worktree. At completion, format there, commit any resulting changes, then run the full Section 9 integration checks on the final candidate. Record the tested commit, toolchain, commands, and results. Any subsequent code change requires repeating affected validation; do not claim that an earlier tested SHA covers a newer one.

After the candidate is frozen, create or update a separate validation worktree at that exact integration SHA. Assign a review subagent to inspect the combined diff against the captured base and review cross-feature behavior and benchmark claims. It must not modify the integrator's worktree. Route required corrections through the integrator or a new correction task, then validate and review the new final SHA as necessary.

Keep one concise execution report on the integration branch, for example `docs/development/library-improvement-results.md`, with task outcomes, completed/deferred scope, validation evidence, and benchmark comparisons. Reports and documentation are committed before the final handoff. Preserve task branches/worktrees until the user has the deliverable; cleanup must not discard unmerged work or remove the user's original checkout.

### 8.6. Final handoff and stop condition

The work is complete only when:

- One integration branch contains every accepted task and all integration corrections; it has no unresolved conflicts or uncommitted changes.
- All required validations have passed on the recorded final commit, or any genuinely blocked check is explicitly reported with its cause. An unvalidated result must not be described as ready without qualification.
- The original checkout still has `master` checked out and this execution has not moved its branch tip or modified its pre-existing working-tree/index state. Verify against the initial record. If the user or another process changed it externally, report the observed difference without overwriting it.
- The final response names the integration branch, its absolute worktree path, base SHA, final SHA, implemented/deferred tasks, test results, and benchmark results/limitations. State that the user performs the merge into `master`.

Stop after handing over that branch. **Do not merge it into `master`, switch the original checkout, fast-forward `master` through a ref command, create a task-by-task merge requirement for the user, or remove the integration worktree.** Publishing the integration branch remotely is separate and requires existing user authorization; a local integration branch is the default deliverable.

Phase 1 commits should be small. In Phase 3, keep local computed-sort concurrency separate from related-target cache redesign. In Phase 5, snapshot format changes require their own compatibility and failure-recovery tests. Do not make unrelated fixes wait for that format work.

For a short first milestone, complete Phase 1 and Q1, then measure on the integration branch. This milestone does not end the overall assignment unless the user explicitly limits scope. If planning dominates, continue there; if build construction is the user's immediate bottleneck, take B2/B3/B1 next. Stop speculative representation changes when their measured benefit is negligible and record the decision in the execution report.

## 9. Validation and benchmark procedure

### Correctness coverage

Use existing reference/oracle and interaction tests as the baseline. Add focused tests where this review identified gaps. Compare IDs, ordering, exact counts, evidence, payload availability, error classes, and stream completion semantics—not only hit counts.

Exercise relevant combinations of:

- Inference disabled, report-only, and automatic indexing.
- Declared indexes, manual opt-outs, complete inferred scalars, and partial short-string coverage.
- Scalar roots, objects, mixed arrays, missing/null/empty selections, and malformed fallback.
- Ordinary search, cached search, page streaming, all-results streaming, estimate-only explain, and analyze.
- Cold preparation, reused preparation, enabled/disabled verified membership cache, and explicit save/restore.
- Locale-aware sorting, ties, both directions, relations, unavailable targets, parameters, and frozen temporal values.

Keep the existing independent relation oracle. Add parser differential testing and property tests for numeric/path invariants where useful. Avoid a combinatorial test suite that simply repeats the implementation; use targeted interaction cases.

Before behavior commits, run workspace tests and Clippy as required by the repository, always from the owning task worktree. At integration, run the following only from the dedicated integration worktree; complete formatting before the final validation and commit record:

```sh
cargo fmt --all
python3 scripts/format-rust-spacing.py
python3 scripts/format-rust-spacing.py --check
cargo test --workspace
cargo clippy --workspace --all-targets
```

Run affected crate tests while iterating. Changes that alter public APIs or serialized state also require building the CLI consumers. This review did not execute these commands.

### Performance workloads

| Workload | What it isolates |
| --- | --- |
| Musenalm, unchanged input and configuration | Real build and query baseline |
| Many small JSON files versus equivalent JSONL | Open/parse cost, file scheduling, framing memory |
| High-cardinality short strings/UUIDs and repeated strings | Inference dictionary memory and inferred-order reuse |
| 1/4/16 conjuncts, range predicates, temporal periods | Repeated planning and dictionary scans |
| First page, repeated pages, deep offset, count-only | Ordering reuse, top-k retention, exact membership |
| Local and related computed sorts, workers 1/2/4/8 | Scheduling, target loading, lock scope, memory reservation |
| Large fanout with shared and dangling targets | Relation construction and target verification reuse |
| Large malformed JSONL | Diagnostic coordinate complexity |
| Many disjoint and overlapping collections | Inventory size and shared-source reuse |
| Large collection snapshot beyond 512 MiB | Snapshot format and temporary-memory limit |

Use release builds, the same dataset, the same machine, and recorded worker/configuration values. Separate warm filesystem cache from cold-cache experiments and separate prepared/verified-cache states. A first sample plus at least five repeated samples is a reasonable initial protocol; increase repetitions if differences are near noise. Report median and spread, not only the fastest sample.

For each retained optimization, report:

1. Starting and final commits, exact command or library harness, input counts/bytes, and options.
2. Build wall time and phase times; inference observed/admitted/dropped paths and coverage; final index bytes.
3. Explicit snapshot bytes and serialization/checksum/write/sync times.
4. Query preparation, planning, execution, first-row latency, total latency, source reads/parses, and result counts.
5. Peak RSS, retained-state estimates, and allocation measurements where relevant.
6. Result parity and any compatibility or memory tradeoff.

Do not compare an in-memory build against a build-plus-save and call the difference an index speedup. Do not claim a gain caused by disabling inference, dropping more fields, changing semantics, removing validation of restored state, or returning less evidence. Gains smaller than normal run-to-run variation do not justify a more complex design.

## 10. Whole-library coverage register

These groups cover all 42 production files. The register records review coverage, not a request to rewrite every module.

| Modules | Assessment / associated work |
| --- | --- |
| `lib`, `diagnostics`, `workload` | Public organization, runtime/benchmark separation, small convention cleanup: O1/O3 |
| `model`, `decimal`, `path_repr` | Preserve exact finite numeric semantics and reversible paths; public numeric invariants and grammar consistency: C5/C6 |
| `query`, `query_path`, `parameters`, `clock_context`, `byte_size_literal`, `temporal_query` | Preserve typed parameters and frozen time; canonical paths and shared execution context: C6/R1 |
| `reference`, `regex_query`, `prepared`, `verified` | Keep semantic oracle and bounded regex preparation; cache replacement/ownership and prepared evaluators: C1/Q6/O2 |
| `executor`, `run` | Highest query-work concentration; estimates, plan reuse, ordering, collectors, materialization: Q1–Q5/O1 |
| `posting`, `posting_values`, `bigram`, `structural` | Keep core index structures and structural semantics; bounded ranges and inferred dictionary access: Q1/O2 |
| `inference`, `inference_build` | Preserve recent allocation improvements; boundary correctness, merge accounting, deterministic early stop: C4/B1 |
| `sorting`, `sort_binding` | Final-status order correctness, inferred order reuse, computed-sort concurrency: C3/Q3/Q4 |
| `relation`, `relation_query`, `relation_semijoin`, `relation_target_cache`, `relation_inspection` | Preserve independent occurrence semantics and inspection; validation duplication, target loading, cancellation: B2/Q4/R1 |
| `source`, `json_scalar_text`, `record_preparation`, `source_access` | Parser parity, bounded framing, coordinate complexity, freshness contract, payload reuse: C2/B3/R2/R4/O1 |
| `discovery`, `inventory`, `build`, `repository` | Construction/publication boundary, budget decisions, sparse inventories and compatible-source reuse: C3/B1/B4/R5/O1 |
| `storage`, `streaming`, `schema_adapter` | Explicit persistence already improved; large sections, typed failures, bounded execution, schema compilation: R1/R3/B4/O3 |

## Source references

All links below are pinned to the reviewed commit. Agents should use the symbols as well as line numbers when applying this plan to a newer revision.

[prepared]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/prepared.rs#L52
[scalar-parser]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/json_scalar_text.rs#L20
[status-refresh]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/sorting.rs#L654
[final-publication]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/repository.rs#L1210
[distinct-bytes]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/inference_build.rs#L175
[number-order]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/model.rs#L90
[field-path]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/model.rs#L514
[query-path]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/query_path.rs
[workload-path]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/query.rs#L261
[sort-expression]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/sort_binding.rs#L97
[planner]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/executor.rs#L476
[boolean-planner]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/executor.rs#L811
[temporal-estimate]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/posting.rs#L368
[explain]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/run.rs#L3422
[inferred-order]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/run.rs#L2393
[inferred-column]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/sorting.rs#L141
[computed-scheduling]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/run.rs#L4209
[related-sort-lock]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/sort_binding.rs#L275
[exact-selection]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/run.rs#L2098
[wildcard]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/reference.rs#L1881
[inference-build]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/inference_build.rs
[relation-validation]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/relation.rs#L144
[file-coordinates]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/source.rs#L882
[full-file]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/source.rs#L800
[snapshot-write]: https://github.com/Simon-Martens/objekte/blob/81788174898c27b2c2c367319bad20ecee450562/crates/core/src/storage.rs#L374


## Implementation report

- **Task:** `library-improvements-20261107`
- **Completion date:** 8 November 2026
- **Reviewed and implementation base:** `81788174898c27b2c2c367319bad20ecee450562`
- **Implementation branch/head:** `integration/library-improvements-20261107` at `4a7e17696e46cf8c1c9968db9af12e8cf1f0cfdc`
- **Completion branch:** `integration/library-improvements-20261107-completion`
- **Completion worktree:** `/home/simon/source/objekte-integration-library-improvements-20261107-completion`

The implementation had already been integrated into the original library branch and merged into `master` before this completion record. The original branch is an ancestor of `master` at `9a94bc90d0d80ff7d14f44e457acf8787212d5ab`. This completion branch forks from the existing implementation branch and adds this task document and report. The original checkout remains on `master` and was not used for builds, formatting, or commits.

The [library improvement execution record](../../development/library-improvement-results.md) lists accepted task heads, integration merges, focused regressions, and measured results for C1–C6, Q1–Q6, B1–B4, R1–R5, and O1–O3. Contributing `improvement/library-*-20261107` branches and non-fast-forward integration commits are enumerated there. The corrected code is `1c64493ab9`; subsequent commits through `4a7e17696e` contain benchmark evidence and reporting, with no changes to core, CLI, TUI, or Cargo inputs. Independent review found and corrected three execution-context gaps before final oracle and benchmark runs.

Validation in this completion worktree at `4a7e17696e`, with Rust and Cargo 1.99.1:

- `cargo fmt --all`, followed by `python3 scripts/format-rust-spacing.py` and its `--check`: passed with no tracked formatting diff.
- `cargo test --workspace`: passed, including core, CLI, TUI, integration and doc tests.
- `cargo clippy --workspace --all-targets`: exited successfully with warning-level diagnostics.
- The previous corrected-code validation also passed a release CLI build. Its independent [jq oracle](../../../bench/results/library-improvements/final-oracle-1c64493/jq/REPORT.md) and [relation oracle](../../../bench/results/library-improvements/final-oracle-1c64493/relations/REPORT.md) compared complete ID sets, passing 300/300 and 18/18 cases respectively.

The [final Musenalm comparison](../../../bench/results/library-improvements/final-musenalm-817-vs-1c64493/README.md) records exact release commands, corpus/schema fingerprints, environment, raw samples and per-query timings for base and corrected code. All 776 rows across 194 queries and four profiles matched answers, page sizes, candidate counts and source-work counters. Sums of uncached query medians changed from 5765.602 to 4010.373 ms (ASCII) and 5641.833 to 4067.079 ms (Unicode); 64 MiB warm profiles were nearly flat. The [large JSONL framing comparison](../../../bench/results/library-bounded-framing-20261107/README.md) measured median opening wall time 493.299 to 430.717 ms and median peak RSS 102.145 to 24.082 MiB on a 100.901 MB fixture over three alternating fresh processes per revision. Focused planning, ordering, count, computed-sort, construction, inference, snapshot and allocation comparisons are linked from the execution record, with exact commands and settings. Benchmarks were not rerun for this document-only completion commit because the measured library code is unchanged.

Measured deferrals and limitations remain explicit in the execution record: B1 worker early-stop needs a deterministic scheduler boundary; B4 shared-source reuse and R5 sparse inventory mapping were not justified by the measured workloads; R3 chunked snapshots were deferred because the measured collection section was below the existing 512 MiB cap. An opaque public prepared-request handle remains deferred pending a generation and clock contract. Query record-batch scheduling for large JSONL files was outside R2 build framing. The snapshot section cap and its precheck allocation remain. Single-process build timings do not establish a cold-cache improvement. No benchmark result is presented as a general workload speed guarantee.

The completed document is committed on this completion branch for the user's merge. This worktree is retained. Earlier subordinate worktrees were not removed because their ownership and retained files were not reverified; no existing worktree was reset or cleaned. The supervisor will remove the original untracked task document only after verifying this committed copy preserves its latest contents and no newer edit exists.
