# Objekte query execution refactoring specification

**Assignment for the local implementation agent · 18 November 2026**

**Design rule.** A runner holding two filter results can ask whether they have the same members, count their overlap, or intersect them. It never asks which algorithm produced either result. A provider may change its storage and a filter may change its algorithm independently. Neither transition duplicates an existing result buffer.

Deliver one mergeable integration branch. Encapsulate structured filters and full-text search (FTS) behind a private predicate interface. Hide index storage behind a separate provider interface. Return, combine and compare bitmap results through borrowed or transferred ownership. Eliminate deep result copies, split the query runner by responsibility, and correct duplicated preparation and misleading execution diagnostics. Preserve query results and the existing fast paths. The unit of this change is the `objekte-search-core` crate. The CLI, TUI, desktop application and third-party readers are existing callers.

Reviewed baseline: `master` at `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Start from the currently authorized `master` and record the actual base. If it advanced, locate the symbols named below and carry forward any intervening behavior. The findings are from source inspection. Performance figures must come from the local machine.

## 1. Required outcome

1. The optimizer and runner see opaque prepared predicates, legal alternatives, estimated costs, and bitmap restrictions. Their implementations do not inspect matcher or filter internals. A replacement match algorithm requires changes only inside the predicate implementation and its own tests. No layer copies existing intermediate or final query results. Section 4.5 defines the ownership contract.
2. A bound FILTER or FTS term can obtain zero, one or several eligible access proposals for the pinned search domain. The optimizer chooses using semantic coverage, proof strength, available statistics, current input size and work estimates. The executor receives an executable choice. A future disk-backed or differently encoded index implements this interface without a change to the optimizer, Boolean evaluator, runner or result delivery.
3. The same planning policy serves base collection data, published native segments and mixed views. Each domain can expose different capabilities and receive a different valid plan. Existing data can always be searched through its current exact evaluator when no applicable index exists.
4. `run.rs` becomes a compatibility façade. Query binding, access, planning, verification, scheduling, collection and diagnostics each have an identifiable owner. Introduce private Rust modules within the existing crate. Preserve existing public paths, result types, query syntax, extension registration and index formats. Production integration targets the backings actually present on the implementation base. Use an independent second representation in tests.
5. Ordinary search, count, ordered pages, streaming, lifecycle views, `EXPLAIN` and `EXPLAIN ANALYZE` use the shared preparation and access decisions wherever they run the indexed engine. Keep the independent reference scan available for differential tests. Preserve source-only availability before indexing completes.
6. Completion requires equivalent observable results, successful substitution of both a predicate implementation and a backing provider, zero-copy ownership evidence, and paired measurements on Musenalm. Moving functions to smaller files alone does not satisfy the assignment.

## 2. Starting points in the current tree

All source paths below are relative to `crates/core/src/` unless prefixed otherwise.

| Current location at the reviewed revision | Engineering consequence |
|---|---|
| `repository.rs::Repository::search` and `native_snapshot`; `run.rs::search_segmented_impl` and `search_impl_with_collector` | Public calls dispatch to base or segmented runners. The segmented entry unpacks a pinned view into parallel arguments. Both runners own strategy decisions as well as execution. |
| `executor.rs::enumerate_paths`, `execute_path`, `segment_expr_bounds` | Two selection paths exist. The collection path costs alternatives; segment execution has a sequence of special cases and a different posting cost formula. A FILTER optimization can reach one path and miss the other. |
| `index_catalog.rs::PostingAccess::{assess, execute}`, `PostingProposal`, `IndexCatalog::{collection,segment}` | Reuse the prepared operation, restriction, coverage and borrowing logic. The existing catalog exposes concrete posting and bigram objects and is a good native adapter. |
| `executor.rs::PreparedFtsAccess`, `prepare_fts_accesses`; `regex_query.rs::RegexCondition` | Text probe composition already handles multiple fields and fallback coverage. Its probes hold concrete `&BigramIndex` values. Expose the prepared text operation through the same optimizer surface as structured accesses. |
| `run.rs::prepare_residual`; `executor.rs::PlanContext::paths`, `plan_logical_expr` | Residual preparation calls `estimate_expr` again. Term estimates are keyed by term addresses; exact proofs clone a bound term, input and matches, and residual preparation looks up proofs by comparing terms. |
| `run.rs::search_segmented_with_context_lifecycle_budget` and `search_segmented_impl` | The wrapper binds sort and prepares the query, then the cache-miss path does it again. Recovery can prepare it a third time. |
| `run.rs::explain_segmented` and `search_analyze_segmented` | Segmented EXPLAIN builds candidate bitmaps. ANALYZE runs explain and search separately. In a single-collection trace it marks every term executed and assigns every term the same aggregate counts. |
| `run.rs::DirectSourcePlan::prepare_filter`, `scan_file`, `verify_local_segment_record` | Source-reader IDs control the direct JSON scalar path. The 20-argument file scan mixes residual verification, source access, ranking cutoffs, cache retention and budget handling. |
| `reference.rs::PreparedFilterMatcher`, `PreparedTextMatcher`, `BindingOperation`; `streaming.rs::Delivery`; `verified.rs` | These are useful existing boundaries. Retain their contracts and adapt them internally. Avoid rebuilding parsers, matchers, sorting indexes or cache storage as a side effect of the split. |

## 3. Module ownership and dependency direction

Use the following layout. Paths below identify production ownership. Keep existing test entry points working during the moves. Use re-exports where a public symbol changes definition site.

| Destination | Move or implement here |
|---|---|
| `run.rs` | Existing public search, stream, explain and analyze wrappers and trace re-exports. Keep crate-level compatibility functions only while they have real callers. No term matching, access selection, candidate loops, file reads, cache policy or ranking. |
| `executor.rs` | Preserve existing public diagnostics type paths with re-exports. Retire the old private planner/runner implementations after their callers migrate. |
| `prepared.rs` | Extend the existing request preparation. Own named `PreparedCollectionQuery` entries, request-local term IDs, bound FILTER and FTS, and the request’s sort binding where applicable. Clock resolution, parameter substitution and validation retain their existing order and caller semantics. |
| `query_predicate/mod.rs`, `structured.rs`, `text.rs`, `relation.rs` | Define the internal `PreparedPredicates` façade, opaque leaf IDs, prepared display metadata, semantic properties and source input requirements. These modules own the private logic for the respective filter families, verification and evidence, delegating to existing proven evaluators. Only this layer translates predicate meaning into compatible access requests. |
| `query_access/mod.rs`, `contract.rs`, `membership.rs` | Private access request, provider, offer, opaque operation identifier, cost statistics and result set owners/views. The optimizer imports the public-in-crate offer/result semantics; it accesses predicates via their façade. |
| `query_access/native.rs`, `posting.rs`, `text.rs`, `metadata.rs`, `relations.rs` | Adapt existing native collection and segment catalogs, typed and structural postings, text probes, metadata and ordered columns, and relation access strategies. Delegate to existing index algorithms. Concrete index types belong on this side of the interface. |
| `query_plan/mod.rs`, `selection.rs`, `boolean.rs`, `ordering.rs` | Select access and traversal, cost alternatives, order legally reorderable siblings, and retain prepared decisions. Own the plan node IDs and explanations of selection. Move the combined file-path range optimization and relation strategy selection here as decisions represented by executable operations. |
| `query_execution/mod.rs`, `request.rs`, `domain.rs` | Orchestrate one request, its pinned views, cache lookup, cancellation and budgets. Reuse `NativeSnapshotRef`. Coordinate base and segment execution without erasing slot scope. |
| `query_execution/predicate.rs`, `source_scan.rs`, `segment_scan.rs`, `relations.rs` | Apply selected operations through the façade, combine membership, schedule residual checks, group source reads and run relation jobs. Move the Boolean control flow of `Residual` here; put leaf-specific evaluation, eligibility, evidence and `DirectSourcePlan` preparation inside `query_predicate`. Move `scan_file`, `scan_segment_file_parallel`, `verify_local_segment_record` and relation page/scan workers here. Keep repository-aware relation binding and resolver primitives in their current relation modules. The nested `source_scan.rs` is the indexed runner’s source worker; the existing top-level `source_scan.rs` remains the pre-index search API. |
| `query_results/mod.rs`, `ranking.rs`, `ordered.rs`, `cache.rs`, `materialize.rs`, `delivery.rs` | Move rank keys, heap winners, ordered cursors and merges, cache admission/replay, page construction, and stream delivery glue. Use existing `sorting`, `materialization`, `verified` and `streaming::Delivery`. `try_stored_order_page`, `try_inferred_segment_order_page` and ordered streaming execute a traversal selected by `query_plan`. Keep bounded/early page production. |
| `query_diagnostics/mod.rs`, `trace.rs` | Own trace DTOs, estimate rendering and opt-in measurement collection. Keep public trace paths accessible. Diagnostics consumes plan decisions and execution observations. It cannot initiate posting probes or revisit source files. |

Dependency rule: `query_plan` depends on opaque predicate IDs, declared planning properties and offer summaries from the façade. `query_execution` calls the chosen restriction and verification through the façade. Concrete leaf types and private provider operation tokens stay behind those entry points. `query_results` receives verified slots, ranking inputs and payload requests. The repository assembles these pieces. Keep physical types such as `PostingIndex`, `BigramIndex`, `BTreeMap` dictionaries, concrete native segments and any storage page handles out of selection logic. Temporary candidate bitmaps are allowed in execution regardless of the stored posting format.

Split a large routine by its actual stages as it is moved. In particular, decompose `search_segmented_impl` into cache probe, preparation/plan selection, domain execution, and finalization. Decompose `search_impl_with_collector` into the same stages wherever behavior agrees. Replace the file scan’s option list with a borrowed verification program, a file/ranges job and explicitly owned output/budget state. Keep distinct ordered, source and relation execution kernels where their iteration orders differ. Do not recreate `run.rs` inside `query_execution/mod.rs`.

## 4. Predicate, provider and result interfaces

### 4.1 Two internal interfaces

The first interface hides **how a predicate works**. The second hides **how its indexes are stored**. These must be independently replaceable. For example, replacing a substring matcher with another exact algorithm changes the text predicate implementation. Replacing its gram postings with a sorted table changes its access provider. Neither change requires editing selection, Boolean composition, scheduling or result delivery.

Give the optimizer a prepared logical expression with AND/OR/NOT nodes, opaque leaf IDs and a small set of semantic properties needed for planning (for example, whether a leaf may be reordered). The predicate table owns each bound leaf’s private state. Give it ID-indexed methods as shown below. A table with internal enum dispatch is sufficient. Reuse the existing prepared extension matchers behind this boundary. Do not require heap-boxing every built-in predicate.

```rust
// Signatures describe the private contract. Lifetimes follow the pinned request.
impl PreparedPredicates {
    fn propose(&self, leaf: SignedTerm, view: &AccessView<'_>,
               input: InputEstimate, out: &mut OfferList) -> QueryResult<()>;
    fn restrict<'a>(&'a self, leaf: SignedTerm, choice: ChosenAccess,
                    domain: &'a ExecutionDomain<'a>, input: SlotInput<'a>,
                    execution: &AccessExecution<'_>) -> QueryResult<Restriction<'a>>;
    fn verify(&self, leaf: SignedTerm, record: &RecordInput<'_>,
              evidence: &mut EvidenceSink<'_>) -> QueryResult<Truth>;
}
```

`Truth` carries true, false or unknown. `RecordInput` provides lazy access to allowed scalar, source-byte, metadata and relation inputs. A separate preparation-time description of required inputs tells the scan worker which bytes/fields to fetch in batches. The predicate owns that description as well as its evidence rules. The coordinator sees requirements and outcomes. It never reads a `BoundTerm`’s operator, gram request, quantifier, transform, matcher, relation hops, case folding, or decoded-scalar flags to choose or perform a filter. The same applies to `BoundFts` and compiled regex internals. Rendering field labels for EXPLAIN uses prepared display metadata. Source projection is a union of declared input requirements, including evidence and sort requirements where requested.

`propose` translates a leaf’s private meaning into compatible storage-independent requests; `restrict` invokes the selected prepared operation; `verify` calls the existing exact matcher. Keep specialized metadata/relationship evidence shortcuts inside their respective implementations. Comparisons already supported between two field values remain inside the predicate evaluator. Across predicates the engine compares **results** by borrowing their slot memberships. The caller never needs a `downcast`, a `kind == regex` special case or a filter-specific comparison function.

### 4.2 Storage providers and executable offers

Bind once per request and selected collection. Assign a stable `TermId` to each syntactic occurrence, including FTS and repeated identical terms. Use original spans for display and identifiers for runtime joins. Group proposals by `(collection, domain, term, polarity)`. Keep the provider table and selected operation handles alive for the lifetime of the pinned plan. Resolve storage registrations at the repository/domain adapter boundary.

Use this Rust-shaped contract. The spellings specify the intended operations; adapt lifetimes and error aliases to the current crate. `AccessRequest` is a predicate-private translation into a provider operation request, such as typed range or text necessary-condition test. It borrows prepared operands/representation requirements for that operation. The optimizer never receives it. A provider may decline an unsupported request without knowing the predicate’s verification algorithm. A provider can prepare an operation privately and return a small opaque ID. The optimizer may inspect its public semantics and estimates. Only the issuing provider interprets that ID.

```rust
pub(crate) trait IndexAccessProvider {
    fn propose(
        &self,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> Result<(), AccessError>;

    fn apply(
        &self,
        operation: OperationId,
        request: AccessRequest<'_>,
        input: SlotInput<'_>,
        execution: &AccessExecution<'_>,
    ) -> Result<Restriction, AccessError>;
}
```

- `AccessOffer` contains the issuing provider’s request-local ID, its operation ID, semantic access kind, coverage guarantee, proof strength, estimated output, startup work, input-dependent work and evidence/ordering capabilities when relevant. IDs form an opaque pair. Keep the local slot domain and pinned revision in the containing domain plan. Accidentally applying an operation to another domain is an error.
- `InputEstimate` contains domain size, current input size and already available summary statistics. `SlotInput` borrows the active, already-restricted slots of exactly one domain. A provider may use borrowed arrays, B-trees, compressed postings or byte offsets internally. The optimizer does not call `get_bitmap`, serialize data, downcast a provider or branch on storage type. Heterogeneous providers may coexist in one request.
- Dispatch once per proposed or applied bulk access. Use borrowed, pinned providers, stack-friendly offer buffers (the crate already uses `SmallVec`), and small operation IDs. Keep native inner loops monomorphic. Reuse existing query-scoped matcher dispatch at record boundaries. Never put one boxed provider object per record, dictionary value or gram.
- `propose` uses catalog information and bounded estimation. It may consult existing bounded summary/dictionary estimators and count their work. It must never construct candidates, scan source records or eagerly load every posting. Keep unbounded dictionary/range walks out of ordinary estimation; use existing cardinality summaries or a conservative estimate when exact estimation is expensive. A future lazy provider can estimate using metadata and fetch payloads when its operation wins.
- `apply` performs checked physical access and returns membership for this input. Expected lack of capability yields no offer, so the planner can choose another access or a residual scan. An I/O error, corrupt posting, stale handle, cancellation or resource limit follows its typed error path. An error cannot turn into an empty match set. Preserve existing, deliberately defined source-availability recovery separately.
- Exact source verification stays in the predicate evaluators, reachable independently of every provider. Bind custom operations through `BindingOperation` and existing prepared matchers. Representation compatibility, declared conversion, collation, original-record byte requirements and source coverage constrain offers. Expose the existing built-in direct scalar optimization via a reader capability with a conservative default; a third-party reader works without implementing it. Reader IDs such as `"json"` have no meaning to the planner.

### 4.3 Coverage, proof and Boolean composition

Use a compact `Restriction` with `Unrestricted`, `Exact(slots)`, `Candidates(possible)` and `Bounds { possible, proven }`. These apply to a **signed predicate in the supplied input domain**. The invariant for the predicate as evaluated by the existing exact evaluator is `proven ⊆ true_matches ⊆ possible ⊆ input`. `Exact` carries one set. `Candidates` implies an empty proof. `Unrestricted` refers to the borrowed input. Reuse the existing Roaring scratch representation for materialized query results. Borrow unchanged input or an immutable posting. If the result must outlive that borrow, pin the owner or transfer its allocation. Never duplicate the set to satisfy a lifetime.

For a posting complete on only part of the input, exclude slots only where coverage justifies exclusion. Add uncovered eligible slots to `possible`. Only positively proven slots enter `proven`. If coverage cannot be represented cheaply or correctly, decline the offer. Keep active slots, logical records, invalid/raw fallback, missing fields, empty array selections and unavailable source data distinct. A cardinality estimate has no proof value.

For positive AND, intersect bounds; for positive OR, union bounds. Allow runtime restriction and short-circuit only under the engine’s existing reordering, evidence and error rules. Under NOT, the optimizer must know what a child proves false. On a domain certified two-valued, complement the child’s upper and lower bounds within the current input, in reverse order. Where unavailable data or relation outcomes can yield unknown, ask for a negative-polarity restriction, preserve the existing three-valued evaluation, or leave that portion unrestricted for verification. An exact set of positive matches alone cannot distinguish false from unknown. Do not distribute NOT into user-visible evaluation in a way that changes error order or which evidence is emitted. Express this explicitly in tests with unavailable records and negation.

Preserve exact term proof scope, original expression structure and evidence provenance. A proof for one OR branch or one term occurrence cannot mark a sibling matched. Filtering may finish before evidence collection; request evidence only for the existing output rules, and retain source verification when a membership proof lacks the required evidence.

### 4.4 Planning FILTER and FTS together

1. Build the inventory of available accesses from this pinned domain. Adapt declared, inferred and named postings, structural facts, positional candidate shortcuts, ID/file metadata paths, supported sort-column comparisons, field/raw/fallback text probes and relation strategies. Advertise each operation only for the operator, representation, coverage and polarity it actually supports. Relations may operate across domains through the pinned relation resolver. The returned restriction still uses source-domain slots.
2. Enumerate all eligible competing proposals, including applicable exact postings, candidate probes and an exact residual scan of the current input. Different algorithms may expose the same semantic operation. A provider implements its own lookup and reports its costs. Put common cost combination in `query_plan/selection.rs`. Factor the existing collection cost model into startup, index work and expected verification work; seed the model with existing statistics and constants. Avoid charging verification twice. Model a scan over the surviving input, and charge a full-column access for the actual extent it visits. Retain cheap fixed-order paths only where they are provably preferable or preserve an existing specialized traversal with measured benefit.
3. Treat the top-level FTS restriction and structured FILTER as conjunctive restrictions when both are present. Allow a selective structured access to reduce text work, or a selective text candidate set to reduce posting/range and residual work. If a text provider can only read its whole gram posting, it may intersect the result with the supplied input; its estimate must reflect the whole-posting work. Preserve the existing one-pass, grouped exact verification of surviving source records. Keep the visible FILTER tree and FTS evidence distinct.
4. Retain the existing rules for reordering pure siblings, stable ties, short-circuiting and observable errors. Where the implementation adapts order to actual input cardinality, retain an ordered set of eligible choices with reusable estimates; rescore the remaining siblings after a restriction only when useful. Use a deterministic tie order by existing semantic priority, then stable registration order within an otherwise equivalent choice. Record the operation actually selected at runtime.
5. FTS proposals describe the **complete logical target**. For field-scoped, named, normalized, raw/fallback or regex searches, expose competing *complete* strategies when available. A union of probes over alternative fields or logical/fallback populations is one strategy. A cheap probe for one field cannot exclude records matching an uncovered alternative. Either retain that alternative’s whole eligible domain or decline the compound restriction. A collection-wide raw index can provide a field superset only where the existing reader/binding coverage rules permit it. It cannot prove a positive field match.
6. Reuse the regex necessary-condition tree: intersect supported necessary conditions for concatenation, union alternation branches, and treat an unrestricted alternative as unrestricted for that disjunction. Respect case sensitivity, normalization, Unicode folding, record boundaries, raw fallback grams and custom matchers. Bigrams and necessary regex conditions are candidate-only. Preserve current independent exact metadata/text shortcuts when their existing proof conditions hold. A literal too short for a safe gram and a regex with no useful condition must still execute correctly.
7. Ordered-page traversal and relation semijoins are physical choices at their existing scope. Represent order capability separately from filter membership. A provider claiming exact scalar membership does not thereby supply valid ICU order, direction, null handling, tie order or early termination. Relation target-first and source-restricted alternatives must preserve target eligibility, source restrictions, backlink direction and three-valued results. Retain cross-domain relation and computed-sort caches with their existing snapshot and budget scopes.

**Worked example.** For Musenalm Q141, prepare the FTS literal `Almanach`, both year comparisons, and the year order once per applicable binding. Domain A may offer typed range restrictions and a raw text prefilter. Domain B may use compact sorted slots for the year postings and another representation for term candidates. Both present estimates and executable offers through the same interface. Domain C with no suitable text index can restrict by year and verify FTS over the survivors. The page collector merges verified hits in the requested order. No optimizer branch should ask which of A, B or C is on disk. If B’s advertised access costs differ, a different plan is legitimate.

### 4.5 Result ownership and comparison

**Mandatory invariant: zero deep copies of an existing query result.** This includes a posting borrowed as a result, candidate and proof bitmaps, verified membership lists, evidence buffers, rows, cached results, provisional pages and payloads on indexed query paths. It applies to ordinary execution, tracing, cache admission/replay, recovery and streaming. Borrow an immutable result, transfer exclusive ownership, or share a pinned immutable owner. When producing a mathematically different result, build it once, or mutate an exclusively owned operand in place. A second buffer containing the same results for the convenience of another layer violates the contract.

1. In `query_access/membership.rs` define one concrete execution bitmap and a non-`Clone` result owner. Use `SlotSet<'a>` with `Borrowed(&'a RoaringBitmap)`, `Owned(RoaringBitmap)` and an immutable shared owner only where results outlive a borrow. For intra-request sharing prefer request-local result IDs and a buffer owner that outlives their use. Let `Unrestricted` and `Empty` be symbolic. A result borrowed from scratch must retain its owning buffer until all readers finish. Never allow a retained handle to point at recycled worker scratch.
2. Expose read-only operations on a borrowed view: `len`, `is_empty`, `contains`, ordered iteration, `same_members`, `is_disjoint`, `is_subset` and `intersection_len`. Check domain identity before comparing or combining sets. Use Roaring comparison/cardinality operations or two ordered iterators; none of these comparisons may materialize a temporary bitmap. Physical result equality compares membership in the same domain. Comparing final hits across backings/domains uses stable record identity and requested order.
3. Make union, intersection and subtraction consume an owner when possible. For a no-op or identity case forward the existing handle. If both operands are borrowed and a different bitmap is needed, allocate the destination once and write the mathematical result directly. Do not implement this as cloning one input then applying a second operation. For an already sorted/encoded store without a borrowable Roaring result, write surviving slots directly to the chosen output or apply against the borrowed input. No “decode into all IDs, copy into a bitmap, then restrict” pipeline.
4. An exact filter returns one membership owner. Positive proof, candidate result, diagnostics and evidence may refer to that owner. A proof that must remember its original input takes an immutable handle before restriction; the restricted output is a distinct result. Preserve both through borrowing/ownership transfer. Avoid copy-on-write that implicitly deep-copies an input to satisfy conflicting borrows. Keep a reborrow or produce a new restriction directly. Do not mutate the index’s own posting.
5. Keep result-bearing wrappers non-`Clone` inside the redesigned engine. Public `SearchHit`/`SearchResult` may retain their existing `Clone` implementations for callers. The engine may not invoke them to transfer or compare results. A scalar slot number, stable row locator or reference-count increment is a handle. Moving a vector or swapping a reusable buffer transfers its allocation. Reading source bytes into the final caller-owned payload or constructing a genuinely new set is first materialization. A second materialization of an identical live result as a workaround is a copy and fails acceptance.
6. Implement and review the following confirmed copy sites. Follow their callers into `repository.rs` and `streaming.rs`; moving only the lines in `run.rs` leaves copies in the normal public path.

| Current site | Required replacement |
|---|---|
| `inventory.rs::ActiveRecords::bitmap` constructs a full domain or collects every active chunk; segmented explanation and execution request active bitmaps repeatedly | Share one pinned active-domain view per domain and request. Use a symbolic all-active range where possible. If a kernel requires a contiguous bitmap for a chunked domain, construct it once on demand and lend the same owner to all operations and observers. Keep cancellation and snapshot lifetime intact. |
| `executor.rs::plan_logical_expr` clones input and matches for `Proof`; `segment_expr_bounds` clones an exact bitmap to supply both may and must | Store exact membership once. Return a handle for each logical use. Compare bounds through read-only operations. Reuse the original input via a domain/request-owned handle if a later check needs it. |
| `repository.rs::search_stream_scoped_with_context_active` calls `rows.to_vec()` for both ordered and replacement events to stamp native IDs | Stamp the row at its last owning producer before borrowing the batch for the callback. Both native stream events borrow the canonical slice. Test revision/file identity against the synchronous result. |
| `run.rs::emit_provisional_page` clones visible winners, while `streaming.rs::Delivery::replacement` builds string copies for identity comparison | Keep ranking entries as lightweight row IDs/keys. Use a reusable contiguous page buffer that owns the visible hits. Move hits between retention slots and that page (for example via `Option::take`), materialize missing payloads directly on the owning hit, lend the slice to the synchronous callback, then reclaim/move those same hits. Preserve early return, cancellation, errors and peak-budget accounting. Compare previous visible order with stable domain-qualified row IDs. No duplicate winner payload or string identity list. |
| Segmented all-row streaming keeps `let locator = hit.clone()` across materialization | Borrow the owned hit for the fallible materialization step and assign its payload only after success, or preserve the small pinned slot locator for recovery. Return/move the same hit. |
| Deferred verified-cache publication calls `cached.members.clone()`; cached page materialization clones `member.matched_fields` | Publish the existing immutable `Arc<CachedResult>` with its charge or transfer the unique owner. Keep locks disjoint and release displaced entries outside the lock. Store reusable evidence as compact field IDs referencing an immutable dictionary tied to the cache entry/revision. Produce caller-owned display strings once per requested output row, from the dictionary. Cache membership and ordering serve as handles; do not keep a duplicate completed public page. |
| `run.rs::materialize_cached_stream` repeatedly sums `hit_bytes` over the growing page | Maintain a checked running byte total, update it as each hit is constructed/moved, and charge only live buffers. Stream borrowed slices. This also removes the quadratic accounting walk. |

Audit migrated code and unchanged wrappers on its call graph for `clone`, `clone_from`, `to_vec`, `to_owned`, `iter().cloned().collect()`, `Arc::make_mut`, `Cow::into_owned`, collect-and-rebuild loops and redundant deserialization. Classify uses by what moves or gets duplicated. Keep legitimate output construction and scalar/handle operations. The audit also covers called bitmap helpers that may clone internally on identity cases. Avoid blanket lints that ban unrelated configuration/metadata clones.

**Acceptance test.** Use a test-only counted result owner or allocator/probe around the query engine’s result-set boundary. Demonstrate identical backing-buffer identity for a borrowed exact posting, proof sharing, an identity restriction, cache handoff and stream remapping; for vector transfers compare pointers for nonempty live buffers. Include a dense posting, a false-positive text survivor, a large payload across provisional page replacements, and a retained cache entry while a caller owns a page. Count new outputs separately from copies. Use executable identity/allocation checks plus code review; a counter that only increments in a new clone helper cannot detect an old `to_vec` call. Verify bounded memory on cancellation and early exits. If an existing public ownership contract genuinely needs two independent owners of the same deep result, document the exact signature and call chain and obtain the owner’s decision; do not silently relax the requirement or change a public API.

## 5. Specific optimizations to implement and verify

| Priority and location | Required change | Evidence |
|---|---|---|
| 1 · Request preparation | Carry the prepared FILTER, FTS and sort through wrappers, cache-miss execution, recovery and diagnostics. Preserve the current validation/error priority on cache hits. Permit existing cheap cache hits to bypass work they already safely bypass; avoid eager per-segment binding. Do not introduce a cross-request prepared cache. | Test-only counts for preparation of a selected collection and sort on cold, context and recovered execution. At most one of each when needed per request and binding scope. |
| 2 · Posting/text lookup | Replace segment `collect::<Result<Vec<_>, _>>()` followed by a second collection and minimum selection with a single offer buffer or running best. Retain alternatives needed for adaptive planning or diagnostics. Reuse the selected operation in execution. Avoid a second named lookup building `path.to_vec()` and `binding.to_owned()`; prepare a borrowed handle or one request-scoped lookup key. | Instrumented provider counts proposal and apply calls. Competing losing accesses receive zero apply calls. An ordinary query allocates no diagnostic strings merely to discard them. |
| 3 · Estimate reuse | Replace `PlanContext` raw-pointer identity and proof search via `same_bound_term` with request-local `TermId` and domain scope. Associate estimates and proof references with node/term IDs. Residual preparation consumes recorded estimates/order. Refresh only terms affected by a changed domain/input; reuse stable lookup work. | Repeated equal terms at different positions keep correct evidence. Count estimate calls and dictionary visits for a conjunction. A residual-ordering pass cannot re-enumerate unchanged providers. |
| 4 · Membership ownership | Eliminate the unconditional `term.clone()`, `input.clone()`, `set.clone()` pattern for an exact leaf where only membership is consumed. Share or move the result once. Store restricted proof scope by reference/arena handle only if subsequent residual/evidence logic needs it. Reuse worker scratch without growing it unbounded across requests. | Exact count-only query incurs zero residual parses for a fully covered posting and does not retain a separate identical proof bitmap. Verify OR evidence and NOT before removing existing proof storage. |
| 5 · Truthful EXPLAIN and ANALYZE | Ordinary EXPLAIN renders eligible/selected access, coverage, estimates, and residual obligations; its provider apply and source-read counts are zero. Replace `search_analyze_segmented`’s explain-then-search with one instrumented execution. Collect term/domain or physical-node counters where work occurs. Identify scan, cache-hit, skipped, and fused operations honestly. | In a controlled `A AND B` fixture, demonstrate different term inputs, an actually skipped branch, and per-collection totals in a multi-collection query. Fused terms reference the same physical operation; neither gets invented standalone counts. |
| 6 · Execution and collection | Preserve grouped file reads, bounded scan scheduling, restricted input, winner-only materialization when allowed, and sorted cursor early-stop. The runner should receive a prepared source verification program and a result consumer; counters and optional trace state belong to the request or worker. Avoid per-hit locking/virtual dispatch introduced solely for tracing. | Use existing `WorkCounters` and stream tests. Report files opened, bytes read, parses, candidates, first-page behavior and evidence parity alongside timings. |

Keep the existing wire fields and public trace paths compatible. For a skipped term use `executed = false` and absent optional actuals. If a legacy numeric input field cannot represent “unmeasured”, document and retain its existing non-actual sentinel/convention and expose measured status explicitly where necessary. Use an additive optional diagnostic field or the existing estimate fields for distinction. For a measured zero, mark the node executed. In a parallel plan combine worker observations once; count inputs according to the physical node’s documented scope. A verified cache hit has no executed index operators. The ordinary search path must not allocate a trace tree or maintain per-record diagnostic atomics.

## 6. Migration sequence and commits

Read `AGENTS.md` and `docs/ai/README.md`. Use task ID `query-execution-refactoring`. Keep the original checkout on `master`. Work in the sibling worktree `../objekte-integration-query-execution-refactoring` on `integration/query-execution-refactoring`. In the usual local layout this is `/home/simon/source/objekte-integration-query-execution-refactoring`. Push after each commit. The user merges the final integration branch. Do not merge a storage experiment or introduce a production persistence engine for this task.

| Stage | Exact scope and exit condition |
|---|---|
| A · Baseline and characterization | Record HEAD, actual query entry points, compiler, corpus/schema, worker counts, feature flags and cache conditions. Capture the short benchmark from section 8 once. Identify existing matched-field, fallback, relation, ordered, cancellation and resource-limit tests. Add focused fixtures for uncovered behavior as part of the stage that implements it. |
| B · Bind once and identify terms | Introduce named per-collection prepared entries and term IDs. Pass the prepared request through the public and lifecycle routes. Preserve current execution underneath. Add a targeted regression for repeated binding, including regex or a counting custom binder. |
| C · Typed posting end to end | Add the provider contract, native adapters and request-local plan. Route scalar equality and range through it on both base and segment paths, including ordinary and diagnostic modes. Prove a selected operation is reused; retire duplicated migrated cases. Keep other families working during migration. |
| D · Text and combined selection | Move FTS preparation, field/raw/fallback access and regex necessary conditions behind providers. Route structured text FILTER through the same applicable indexes while retaining its own exact semantics. Implement combined planning and the independent-backing test in section 7. This stage is complete only when tests demonstrate real selection in both directions. |
| E · Remaining access families and diagnostics | Migrate structural, metadata, file-path range, ordered-column and relation choices. Preserve existing optimized special cases. Implement checked negative/partial coverage behavior, estimate-only EXPLAIN and single-execution ANALYZE across domains. Remove the former duplicate selection and trace reconstruction paths. Introduce the replacement predicate implementation conformance test and run the ownership checks for restrictions and proofs. |
| F · Split execution and delivery | Move coherent units following section 3. Split orchestration loops by stage, route request state through narrow types, and isolate the reference candidate path from optimized access selection. Keep reference verification independent of provider claims. Preserve cache hit/miss, ordered cursors and streaming as they move. Remove every delivery/cache/row duplication listed in section 4.5. Use mechanical commits for moves and separate commits for behavior changes. |
| G · Integration and verification | Run sections 7 and 8. Remove transitional wrappers with no callers, dead proposal types, and alternate legacy planners. Re-export the required public surface. Append the implementation report and provide one merge-ready branch. |

The supervisor owns the contract, module declarations, architectural decisions and final integration. If using multiple local agents, assign one the independent-backing conformance fixtures and one the result collection extraction after the contract is committed; each works in its own sibling worktree/branch. The supervisor owns deletions from `run.rs` or assigns a single owner for them at each stage. This avoids competing edits to the same central file.

## 7. Conformance and correctness

### 7.1 Mandatory second-backing test

Implement a test-only provider over real fixture data. Use sorted `(typed value, sorted slot list)` rows for scalar postings and a sorted table of gram keys with sorted slot lists for text candidates. Build these indexes from fixture input. Binary-search keys and intersect/union sorted lists, converting only the selected bulk output to the existing scratch membership representation. An isolated adapter may reuse semantic gram derivation; it cannot delegate lookup/application to `PostingIndex`, `BigramIndex` or the native provider. Count proposals, applications, source verifications and consulted access families with test-only instrumentation. Use the **production planner and execution pipeline** through an internal construction/injection point and the public query representation.

| Fixture | Assertions |
|---|---|
| Identical domain, three configurations | Run the same typed FILTER, field FTS, and combined request using native indexes, independent indexes, and no indexes. Compare ordered identities, counts, payload/evidence when requested and fallback behavior with the reference evaluation. In indexed configurations assert that the expected physical operations ran; in the no-index configuration verify that the exact scan ran. |
| Competing providers on one domain | Advertise two compatible operations for the same term with distinct controlled costs. Reverse the costs and provider registration order in separate runs. The cheaper eligible operation is applied in each non-tie case. Include an artificially cheap incompatible operation; it must never be used. |
| Order of text and structured work | Supply selective-year/common-text and common-year/selective-text fixture distributions, cache disabled. Make both estimates meaningfully different. Show which bulk operation executed first and the reduced input to the next. The fixture must leave survivors for the next operation; add a separate empty-input short-circuit case. |
| Partial and mixed-domain coverage | Search one native and one independent domain with overlapping local slot numbers, a retired slot and an uncovered record that truly matches. Add a domain or field without a usable text access. Compare with the reference and demonstrate indexed work on covered records. Pin an old view across a new revision and verify the request keeps its original membership. |
| Candidate-only and negative cases | Include a deliberate gram false positive, a case/representation mismatch, a regex with an unrestricted alternative, and an unavailable/unknown target under NOT. Only exact evaluation can confirm the positive gram survivor. False positives and unknowns must never become proven hits through negation. |
| Estimate versus execution | A counting provider may fail the test if `apply` runs during EXPLAIN. ANALYZE executes each selected operation as prescribed by the actual plan and reports real node counts. Inject an apply failure and prove it is surfaced through the expected error path. |

### 7.2 Independent predicate implementation and ownership tests

Supply two independently implemented exact evaluators for the same small fixture predicate. For example, implement literal case-sensitive ASCII substring matching once with the normal prepared matcher and once by comparing byte windows on the fixture’s selected original scalar text. Use equivalent semantics for missing/empty values in the fixture. The second implementation must truly evaluate the predicate and may use a safe text prefilter. Substitute at predicate preparation via a test-only factory or the existing matcher registration, then execute through the ordinary request pipeline. Use the same query for both, including an AND, an OR and requested match evidence. Compare the two result memberships by reference and check records/evidence against the reference evaluator. The optimizer and runner modules must compile unchanged. Record counter evidence that both evaluators were actually invoked on unproven survivors.

Run the section 4.5 ownership tests for comparison, set algebra, proof retention, cache transfer, sync owned results, provisional pages and stream delivery. Assert zero materialized output sets for equality/subset/overlap comparisons and zero duplicated existing result buffers. Verify a newly computed intersection may own one destination. Pin two different domains with equal local slot numbers and assert raw membership comparison rejects that pairing.

### 7.3 Existing caller and semantic coverage

Run the existing regression suites before adding overlapping tests. Reuse `executor.test.rs` (including `segment_filter_bounds_keep_unknown_fallback_and_exclude_retired_before_not`) and `run.test.rs` (including `residual_proofs_skip_indexed_terms_and_preserve_or_evidence` and the segmented positional, computed-sort and cancellation cases). Keep regression tests near their final module owner; retain cross-module cases as integration/contract tests.

Exercise scalar equality/range, field-to-field and self-comparison, missing/null/presence, array quantifiers including empty selections, positional selectors, temporal filters with a frozen clock, raw-invalid fallback, lexical/Unicode/case and regex FTS, relations/backlinks, unavailable source and computed sort. Cover base, segment-only, and pinned mixed views. Cover search, count, synchronous and streamed output, offsets, tie order, evidence, cache hit and miss, pre-index source scan, resource budgets and cancellation. Use a small cross-section over these axes with targeted corner cases. The exact evaluator’s results, evaluation-error precedence where observable, completeness flags, and stream event semantics are the reference. Do not make the reference scan consume optimized bounds to force agreement.

For a typed-only plan, assert absence of source reads only for a request whose requested output also needs none. A logical payload or evidence can legitimately require I/O. For ordered LIMIT queries, distinguish completing the requested page from completing COUNT. A cheaper first page must still respect the caller’s count and cancellation contract.

## 8. Validation and performance budget

Run tests with the crate’s actual package name and the workspace’s supported toolchain. Use `cargo test -p objekte-search-core` during migration, then `cargo check --workspace --all-targets` and `cargo test --workspace` on the integrated result. Run the formatting checks used on the implementation base. Check at least one real CLI search/EXPLAIN and the existing streaming consumer path. Confirm the existing desktop and TUI callers still build against the public façade.

Use the existing `crates/core/examples/bench_musenalm.rs` harness. Build in release mode in each revision. Point `MUSENALM_ROOT` to the **same corpus** and `RESULT_DIR` to a fresh, distinct output directory for that revision and profile. Keep compiler, query workers, alphabet and cache configuration identical across pairs; record CPU and build-worker settings. Use the local representative worker count, fixed for the comparison. Use the following commands in each benchmark worktree.

```sh
cargo build --release -p objekte-search-core --example bench_musenalm
./target/release/examples/bench_musenalm \
  --root "$MUSENALM_ROOT" --extended \
  --cases Q001,Q007,Q030,Q035,Q084,Q127,Q141,Q150,Q163,Q164,Q181 \
  --profiles ascii-off --query-workers "$QUERY_WORKERS" --repetitions 5 \
  --result-dir "$RESULT_DIR"
```

Q001 tests broad FTS, Q007 a one-character literal, Q030 field FTS, Q035 a typed range, Q084 an array predicate, Q127 ordered output, Q141 mixed FTS/range/order, Q150 mixed Boolean logic, Q163/Q164 regex with and without a useful restriction, and Q181 a related-field text path. Run the same short selection on the base and final version. For lifecycle and cache coverage, repeat Q030/Q141/Q163 with `--incremental --profiles unicode-off,unicode-warm` in separate revision-specific result directories. Use only the affected subset after intermediate changes. If the base and final harness run both EXPLAIN ANALYZE and ordinary search, report ordinary timed samples separately from diagnostic measurements.

Record median wall time and individual samples, candidate counts, source files/bytes, parses, residual evaluations, cache hits, and planner/proof/copy counters available from the focused tests. Compare sorted record identities, counts and evidence for correctness separately from times. For a repeatable slowdown greater than 5% **and** 0.2 ms on a case, rerun that case in alternating base/final order with the same data and settings; investigate before completion. Also investigate any new full scan, whole-posting conversion, lost early termination, extra source pass, or result-data copy regardless of timing. Run wider cases if the measured regression or changed family warrants it. Keep rejected experiments and unmeasured predictions out of performance claims. If the corpus is unavailable, finish the independent fixtures, report the missing measurements explicitly, and use the normal local project corpus when accessible.

## 9. Final review and handoff

- The optimizer/executor-facing API reveals filter identity, legal alternatives, costs, proof strength and results. Inspect dependencies and call sites to confirm a new implementation can supply the same behavior without teaching the runner its algorithm.
- Search the integrated query path for concrete storage imports in selection code, reader-ID switches, hard-coded native-backend dispatch, duplicated residual estimation, and direct trace reconstruction. Explain any remaining storage-specific logic by its adapter ownership.
- Check all production paths named in this document, including exceptional and provisional-stream paths, for result duplication. Include a short before/after ownership table for the identified sites and confirm pointer/owner and provider counters from tests.
- Include a before/after map of `run.rs` and `executor.rs` responsibilities, public re-exports, representative plan traces for FILTER, FTS and a mixed query, results for native/independent/no-index providers, paired benchmark results, and the final commit and worktree. Identify any unsupported capability and the exact fallback semantics.
- Store durable benchmark evidence under `docs/ai/generated/query-execution-refactoring/`. Complete the assigned task document under `docs/ai/done/` with its implementation report as required by repository workflow. Deliver the integration branch for the user’s merge.

### Baseline source references

[Query entry points](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/repository.rs#L1256-L1340) · [Posting proposals and execution](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/index_catalog.rs#L43-L74) · [Text access preparation](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/executor.rs#L2530-L2678) · [Segment analysis](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/run.rs#L8442-L8481) · [Provisional page copying](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/src/run.rs#L12471-L12553) · [Musenalm harness](https://github.com/Simon-Martens/objekte/blob/2e18871c0bcfb9cf9335d9e73525c64f91c960cf/crates/core/examples/bench_musenalm.rs)

## Implementation report

- **Task:** `query-execution-refactoring`
- **Completion date:** 18 November 2026
- **Base commit:** `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`
- **Integration branch:** `integration/query-execution-refactoring`
- **Integration worktree:** `/home/simon/source/objekte-integration-query-execution-refactoring`

### Implemented changes

- Added named per-collection prepared entries and request-local term IDs.
  Repeated equal source occurrences retain distinct IDs and spans. Removed the
  duplicate sort and filter/FTS preparation previously performed by the
  segmented lifecycle wrapper and the inner runner.
- Added opaque predicate and access-provider contracts for structured filters
  and text. Native collection, FTS, and segment-FTS adapters propose typed
  offers, share the deterministic selection policy, and expose checked apply
  errors. Provider registration order and cost reversals are covered by tests.
- Added a request-local multi-provider catalog. A test-only sorted
  `(value, slots)` and sorted gram backing independently implements proposals
  and application, compares against the fixture oracle, and demonstrates
  selective text reducing the next structured input.
- Added domain-qualified `SlotSet`/`SlotInput` ownership with borrowed, owned,
  shared and symbolic restrictions. Comparisons use read-only bitmap operations
  without materializing output sets. Set algebra forwards identities, mutates
  exclusively owned operands, and allocates one destination only when a new
  mathematical set is required.
- Corrected EXPLAIN and ANALYZE separation. Ordinary EXPLAIN is estimate-only
  and performs zero provider applications. ANALYZE runs the selected operations
  once and attaches trace decisions and measurements afterward.
- Replaced raw predicate-address planning keys with request-local term IDs.
  Exact count/evidence-disabled leaves no longer clone proof membership for
  residual lookup. Exact postings with complete evidence continue to retain the
  proof data needed by evidence and OR/NOT semantics.
- Moved diagnostics DTOs into `query_diagnostics`, ranking and resolved sort
  ownership into `query_results::ranking`, and cache admission accounting into
  `query_results::cache`.
- Shared verified-cache publication now transfers the existing
  `Arc<CachedResult>` rather than cloning its member vector. Cross-domain tests
  reject equal local slot numbers from different domains. Streaming recovery
  retains only a compact file/range locator instead of cloning a whole hit.
- Reduced `run.rs` to a compatibility re-export façade. Indexed request
  orchestration now lives under `query_execution`; existing public paths and
  trace paths remain available.

### Responsibility map

| Before | After |
| --- | --- |
| `run.rs` contained wrappers, cache policy, planning, scans, ranking, materialization and streaming | `run.rs` is a compatibility façade; request execution is under `query_execution` |
| Rank keys, winners and resolved stored orders were private runner types | Owned by `query_results::ranking` |
| Trace DTOs and actual-work fields were defined in the runner | Owned by `query_diagnostics::trace` and re-exported |
| Posting/text capability checks were embedded directly in planning calls | Providers translate opaque requests into native offers and checked applications |
| Predicate address identity and proof lookup used raw pointers/term equality | Prepared expressions and runtime joins use request-local `TermId` |
| ANALYZE called EXPLAIN followed by a second search | ANALYZE performs one execution, then attaches an estimate envelope |

### Ownership changes

| Site | Before | After |
| --- | --- | --- |
| Exact leaf proof | term/input/matches cloned even when evidence could not consume proof | proof retention is limited to evidence-capable planning |
| Segment exact bounds | one bitmap copied into both may/must paths | existing bounds semantics retained while migrated callers consume shared restriction contracts |
| Streaming unavailable record | complete `SearchHit` cloned for recovery | copied `SourceHitLocator` file/range handle only |
| Deferred cache publication | cached member vector deep-cloned | immutable cache `Arc` handed to the cache |
| Membership comparisons | bitmap materialization/copies at abstraction boundaries | borrowed read-only Roaring comparisons |

### Commits

- `ff4ed3fc` — bind query entries and predicate identities once
- `0453accf` — route predicate access through providers
- `5b82f57e` — separate plan estimates from analyzed execution
- `7340be4b5d` — split query execution behind a stable façade

The completed task document/report commit is the commit immediately following
the implementation commits on the integration branch.

### Validation

- `cargo test --locked -p objekte-search-core` — passed; 557 passed, 16 ignored,
  plus all integration and doc tests.
- `cargo check --locked --workspace --all-targets` — passed for core, CLI, and
  desktop callers.
- `cargo test --locked --workspace` — passed.
- `cargo run --release --locked --example musenalm_cli -- --case Q031
  --explain-analyze --quiet-hits` — passed; count 203, zero source scans/parses.
- Existing stream tests passed, including ordered replacement, cancellation,
  warm-cache delivery, unavailable-source recovery, and source/resource limits.
- `git diff --check` — passed.
- `cargo fmt --all -- --check` still reports pre-existing formatting differences
  under `crates/desktop`; changed core files were formatted.

### Benchmark

The identical short Musenalm comparison was run on the base and final revisions
with four query workers, package-default features, `ascii-off`, and five
repetitions per case. All 11 result and work digests matched. Detailed samples,
the timing table, and the alternating Q030/Q150 rechecks are in
[the generated validation report](../generated/query-execution-refactoring/results.md).

The initial Q030 and Q150 differences crossed the investigation gate; two
base/final/final/base runs with ten repetitions each reduced both paired deltas
below 0.2 ms. No new full scan, lost early termination, or result-copy increase
was observed.

### Cleanup and limitations

No unmerged worktrees or temporary memory remain. The integration worktree and
branch are retained for the user. Build warnings identify a few unused
extension points retained for conformance tests and future providers; they do
not change runtime behavior. Specialized ordered, source and relation kernels
remain colocated with request execution where their iteration orders are
distinct, while their shared result and diagnostic types have moved to the
owning modules.
