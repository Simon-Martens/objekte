# Objekte: staged index construction, compact field states, and query readiness

Prepared: 12 November 2026  
Task identifier: `staged-index-construction`  
Integration branch: `integration/staged-index-construction`  
Integration worktree: `/home/simon/source/objekte-integration-staged-index-construction`  
Intended repository location: `docs/ai/tasks/objekte_staged_index_construction_specification.md`

## 1. Objective and governing decisions

Optimize time to usable file listing and full-text search. Divide initial construction and rebuild into two source passes:

1. **Base construction:** discover files, establish stable identities and record locators, perform required validation, and construct the bigram indexes and indispensable metadata needed for correct listing and text search.
2. **Extended construction:** reread source files in bounded batches and build the selected field indexes, explicit inference results, and compact field type/value states together.

Publish a searchable base before starting extended construction for that opening/build scope. Searches must work while extended indexes are absent, partially covered, building, paused, cancelled, or failed. A query uses the capabilities actually published when its execution starts and evaluates unresolved records from source. Index readiness changes performance, not query meaning or completeness.

Do not create a third source pass for compact field states. Accept the deliberate extra read/parse in pass 2; do not retain the corpus of parsed documents, introduce an intermediate spool, or build a speculative concurrent pipeline to eliminate that read in this task.

Field-selection policy is explicit:

- With an index configuration and inference disabled, create compact states only for the fields selected by that index configuration.
- With no selected fields and no explicit inference request, build no general field-state index.
- An explicit inference request discovers eligible fields and sub-fields and builds compact states for all of them, alongside existing opt-in inference behavior.
- Preserve current full-value index admission thresholds, explicit hints, exclusions, and configuration precedence. Adding compact states does not authorize a full value dictionary for every discovered field.
- Query syntax works independently of whether the queried field has an index.

**Unconfigured repositories with inference disabled build only the base search index:** bigrams plus indispensable IDs, locators, validation state, and listing/search metadata. Build no general field-state, presence/null, length, typed-value, or sorting indexes implicitly. Skip pass 2 entirely when no extended capabilities are requested; do not reread the files for an empty extended job. Field predicates remain usable through source evaluation.

Implement `:?`, `:string`, `:number`, `:array`, `:null`, and `:object` predicates. Preserve `:exists`. Eliminate the proposed `()` state entirely: no unit/tuple value, syntax, storage code, or migration is introduced.

This is an implementation task for core, persistence, CLI/TUI where applicable, and the existing native desktop. A backend demonstration with unchanged blocking application startup is incomplete.

## 2. Dependencies, inspection, and execution workflow

Base this task on the completed unified planner and predicate-ordering/shared-evaluation work:

1. `objekte_unified_query_planner_specification.md`.
2. `objekte_predicate_ordering_and_shared_evaluation_specification.md`.
3. Current implementation and completion reports for the index lifecycle and its repairs.
4. Current desktop lifecycle integration and navigation contracts.

The two planner tasks must be complete before dependent implementation. Record their final commits and this task's actual starting commit. Preserve improvements already present instead of implementing a parallel planner or evaluator.

Repository access was unavailable when this document was prepared. The locations below are inspection targets from earlier verified work, not a claim about the current contents of master. Phase 0 must trace the actual implementation, including later merges.

Inspect:

- Core construction, discovery, source adapters, validation, extraction, bigram assembly, and configuration resolution.
- `crates/core/src/run.rs`, `executor.rs`, the delivered common catalogue/planner, postings, inference, and storage.
- `lifecycle.rs`, mutation, maintenance, compaction, persistence, task progress, and revision ownership.
- Current CLI argument parsing, open/build/query paths, batch execution, and TUI startup.
- `crates/desktop/src/backend/open.rs`, `backend/query.rs`, `backend/index_lifecycle.rs`, `controller/runtime.rs`, `ui/state.rs`, `ui/model.rs`, and index/opening panels, or their replacements.

### 2.1 Integration branch and worktrees

This task uses a separate integration branch and worktree. This instruction supersedes the earlier direct-on-master instruction for this task. Keep the original checkout on `master`, with unrelated staged and unstaged changes untouched. Do not implement, commit, or benchmark task changes by modifying that checkout.

- Integration branch: `integration/staged-index-construction`.
- Integration worktree: `/home/simon/source/objekte-integration-staged-index-construction`.
- Subtask branches: `task/staged-index-construction/<scope>`.
- Subtask worktrees: `/home/simon/source/objekte-task-staged-index-construction-<scope>`.
- All worktrees are siblings under `/home/simon/source`; never use `/tmp`.

Start from an explicitly recorded completed prerequisite commit. If prerequisites have not reached master, base the integration branch on the completed prerequisite integration commit and document the dependency/merge order. Do not merge prerequisites into master yourself. Dependent implementation must not race unfinished predecessor work.

Push after every implementation commit and integration merge. Do not reset unrelated work or force-push. Follow `docs/ai/README.md`: preserve uncommitted task documents while active; at completion commit the latest task document with its implementation report under `docs/ai/done/` on the integration branch. Keep the integration branch and worktree for the user's review.

**Merge into master only after the user explicitly commands that merge.** Passing tests, completing benchmarks, or receiving the implementation assignment does not authorize it. Disable/do not configure automatic merging for this task.

### 2.2 Integrator ownership and delegation

The integrator owns the integration branch, shared contracts, merge sequence, combined validation, and final benchmarks. Subagents work in their own branches/worktrees and never edit the integration worktree concurrently. Agree on API and file ownership before parallel edits; dependent work waits for a usable recorded contract.

| Owner/scope | Responsibility |
| --- | --- |
| Integrator | Baseline, shared readiness/revision contracts, cross-cutting review, integration merges, final benchmarks and report |
| `field-states` | Predicate semantics, compact encoding, storage/capability adapter, focused correctness tests |
| `construction` | Two-pass scheduling, readiness publication, partial coverage, lifecycle and persistence integration |
| `cli` | Command wait policy, inference actions, process lifetime, output and TUI integration |
| `desktop` | Progressive opening, status/controls, predicate builder, native interaction validation |

Delegate only independent work after its prerequisite contracts exist. The integrator may implement shared pieces directly and combine scopes where that avoids overlapping edits. Require focused validation before each subtask merge, resolve conflicts on the integration branch, and run combined checks after integration. Do not deliver unmerged subtask branches as the finished result.

The integrator must run and assess the final benchmark suite on the combined integration commit. Individual subagent measurements are supporting evidence, not a substitute. Use an isolated baseline worktree under `/home/simon/source` if needed; never switch or modify the original master checkout to obtain the baseline. Serialize benchmark runs and stop competing build/index jobs so measurements are comparable.

Deliver working behavior in phases. Do not ask permission for routine reversible implementation decisions already within this scope. Record concrete incompatibilities rather than inventing APIs. Do not claim tests or benchmarks ran when the environment prevented them.

## 3. Readiness and identity model

### 3.1 Distinguish listing, searching, and acceleration

Expose independent readiness facts, not one global ready/busy Boolean.

| State/fact | Meaning |
| --- | --- |
| File catalogue available | File identities and known listing metadata can be displayed; discovery may still be progressing. |
| Record locator available | This record's boundaries and source association have been established. |
| Search ready | Base construction for the requested scope has completed sufficiently to execute correct searches under the existing source-error contract. |
| Extended indexing pending/building | Additional acceleration has not finished; search remains available. |
| Extended indexing complete | All requested capabilities have finished for the applicable current domain. |
| Extended indexing partial/paused/cancelled/failed | Published coverage remains usable; uncovered records require fallback. |

A progress percentage is not capability coverage. A capability can be complete with zero matches. A stopped task can leave useful partial coverage. No requested extended indexes means extended indexing is unnecessary, not stuck at zero percent.

Listing readiness must not require parsing every file or collecting every sort key. Reuse discovery and catalogue data; do not add a separate preliminary filesystem enumeration solely for a progress denominator.

Stable file IDs may be assigned as discovery accepts entries. Record IDs for multi-record files become available when record boundaries are known. Do not fabricate record IDs from file IDs, assume one record per file, or present provisional offsets as valid locators. Distinguish file and record counts.

For an initial open, permit catalogue display during base construction. Ordinary searches start at SearchReady. Do not present a partially discovered corpus as a complete search result. When rebuilding an existing session, continue searching the previously published view until its replacement base is ready.

### 3.2 Separate semantic data changes from capability changes

Maintain distinct logical identities for:

- Index/session instance.
- Data revision: record membership, content identity, routing, schema/binding semantics, and other facts that can change answers.
- Capability revision: available indexes, coverage, and physical access structures for the same data.
- Operation/task identity and progress sequence.

Reuse existing identifiers where possible; add only the missing distinction. Publishing another field-index batch must not look like a content edit.

Each query captures a coherent data view and capability snapshot at start. Workers, counts, continuations, exports, and relation-local evaluation remain attached to that view. New queries may use newer capability coverage. Do not change a running physical plan underneath its iterator.

A capability snapshot is immutable to readers. Publish roots/references atomically, with expensive extraction, compression, serialization, and destruction outside publication locks.

## 4. Pass 1: make listing and search usable

Pass 1 includes only work required for the base search contract:

- Resolve source/configuration semantics once using existing explicit settings.
- Discover files and collection memberships; assign stable identities through the existing lifecycle.
- Establish record boundaries/locators, including multi-record files.
- Perform required format and declared-type validation, including malformed skipped fields and trailing input.
- Determine valid logical extraction versus broken/raw fallback under the existing policy.
- Extract the configured searchable text and build its required bigram representations.
- Populate indispensable catalogue metadata, active-record state, validation status, and source/binding identity.
- Retain minimal identifiers required for existing record lookup or relation semantics where those cannot be obtained by a correct fallback.

Do not defer declared schema semantics or change validation outcomes merely to make SearchReady earlier. Validation belongs to construction/update, not ordinary search.

Do not construct optional typed postings, full value dictionaries, array-length postings, optional sorting indexes, inferred field inventories, or compact field-state columns during pass 1. Audit hidden work in constructors and shared extraction helpers: moving the outer function call while its expensive preparation still runs in pass 1 does not satisfy the task.

Collection-wide and field-scoped bigram representations already required by the configured text-search contract remain part of pass 1. Any text binding without usable grams must retain its conservative scan route; an empty bigram set is not proof that no text exists.

If an adapter currently needs a logical Value to validate/extract, retain it only for the current bounded work unit. Reuse borrowed strings and scratch. Do not retain parsed records for pass 2. Preserve existing extension behavior; report unavoidable adapter-specific pass-1 costs.

Publish SearchReady before optional extended work or optional full snapshot persistence. The publication must be operational: a query executed while pass 2 is held at a test barrier must succeed.

An empty collection reaches SearchReady with an exact empty domain. Unreadable scopes and malformed files follow the established typed error/fallback contract; neither is silently converted to absence.

## 5. Pass 2: build all selected extended indexes together

### 5.1 Scope and traversal

After SearchReady, start pass 2 only when extended work is requested. Otherwise mark extended indexing unnecessary and perform no second source read/parse; an explicit wait-for-complete request is already satisfied by the completed base. For requested work, enumerate the published record/file catalogue rather than rediscovering the filesystem. Use the selected fields and explicit inference mode resolved for this build.

Read each required physical file once per normal pass-2 processing unit and parse its applicable records once. Compute configured postings, lengths, sort keys, existing inference observations/admission, and compact states from that traversal. Preserve file-level batching for JSONL/multi-record sources; do not reopen the same file once per record or once per indexed field.

Do not add one source pass per index family. Retries, explicit later inference, updates, and subsequent queries may read sources again; account for those separately from the normal two-pass construction contract.

For very large files, stream bounded record/byte batches and avoid retaining the whole file or document tree. Reuse worker-local source, decode, normalization, and temporary collection buffers with existing size-retention limits.

A source adapter may perform necessary parse checks while reparsing. Do not add a redundant independent validation pass. Any pass-2 parse/read failure leaves its affected capability coverage unresolved and preserves the published base.

### 5.2 Publication units

Publish completed batches with:

- Index data.
- Exact covered-record identities and relevant selection semantics.
- Statistics and proof limitations.
- Data/configuration identity used to build them.
- Updated capability revision and task status.

Data and coverage must become visible together. Never mark a record covered before all occurrences needed by that capability were processed.

Use reusable immutable segments/blocks and existing incremental structures. Do not rebuild or deep-copy the entire collection after every batch. Avoid unbounded growth in tiny segments; use the existing bounded merge/compaction mechanism or a narrow policy with measured query cost.

Choose batch limits by both bytes and records. Make cancellation and publication latency bounded by the current work unit. Coalesce progress updates; a display notification is not a reason to publish or serialize a new index root.

### 5.3 Resource scheduling

Use the existing bounded executor. Give foreground query work and necessary lifecycle updates priority over extended construction. Bound pass-2 concurrency, input queues, output buffers, and pending publication memory.

Do not assume OS thread priority alone protects query latency: disk bandwidth and memory pressure matter. Schedule additional batches according to the shared resource budget and yield at safe boundaries. Derive worker limits from available resources rather than one thread per file or a fixed large worker count.

The first implementation may use a conservative fixed background budget with foreground-aware admission. Do not build a general adaptive scheduling framework.

Pass 2 must not start early to hide its work under pass 1. The selected architecture deliberately separates the initial critical path. New-record base updates while pass 2 runs are allowed and take priority.

## 6. Exact field selection and inference policy

Resolve field selection once into the existing canonical path/selector model.

| Configuration/request | Compact-state scope | Other extended indexes |
| --- | --- | --- |
| No index configuration; no inference request | None | None; base bigrams and indispensable metadata only; skip pass 2 |
| Configuration present; inference off | Exactly fields selected for indexing by that configuration | Exactly configured capabilities |
| Explicit inference requested | All eligible discovered fields and sub-fields | Existing inferred-index admission policy plus configured capabilities |
| Later explicit inference on a searchable session | Discover and populate uncovered eligible paths in a new extended build | Extend existing indexes without rebuilding unchanged bigrams |

A type declaration by itself is not an indexing request unless that is already the repository's documented configuration meaning. Selecting an object field does not silently select all descendants; follow explicit configuration expansion rules. Explicit inference is the action that broadens discovery.

Audit legacy default constructors that create presence/null, empty-string, or array-length postings merely because they observe a field. They must not create those structures for an unconfigured, inference-disabled repository. Such predicates use source fallback until explicit configuration or inference selects their acceleration. Preserve indispensable base identity/validation metadata; it is not a general field index.

Preserve hard exclusions already defined by configuration. Report excluded paths as excluded, not absent or fully covered. “All fields” means every eligible logical path, including nested object/array-valued fields; do not apply low-cardinality string admission thresholds to compact state recording.

For inference, canonicalize repeated array paths according to the query resolver. Do not create a distinct inferred field for every numerical array position. Explicit configured positional selectors remain distinct where supported.

A path discovered late in pass 2 does not require reparsing earlier records if their complete traversal already proves it absent. Reuse shared traversal coverage and record that proof. If earlier extraction was selective, those records remain uncovered for the new path. Do not backfill absence without evidence.

Missing catalogue entries are not automatically globally absent fields. An unknown query path can be proved absent only where complete field discovery/traversal establishes that fact; otherwise use source evaluation.

Bound discovery metadata and construction memory. On a resource limit, report partial coverage and a resumable/failed task; never silently drop fields and advertise completion. Prefer bounded storage growth over arbitrary dataset-size ceilings.

Inference requests and their effective policy belong to the session/build configuration. Persist that policy with a saved index when supported, but do not modify source schemas or repository configuration files automatically.

## 7. Query language and evaluation semantics

### 7.1 Required syntax

Implement these forms through the normal parser, binder, reference evaluator, planner, diagnostics, and serializers:

```text
FILTER annotation:? = true
FILTER annotation:? = false
FILTER annotation:string = true
FILTER amount:number = true
FILTER contributors:array = true
FILTER annotation:null = true
FILTER details:object = true
```

Support Boolean equality and inequality consistently with existing predicate syntax. Boolean negation and composition use the existing grammar. Bare modifier shorthand is supported only if the language already has a general Boolean-expression shorthand; do not invent a second shorthand grammar.

These modifiers are predicates with Boolean results. Invalid operands, unsupported ordering comparisons, invalid modifier chains, and parameter type mismatches receive normal binding errors. Existing metadata syntax, escaped field names, regex parsing, and literal question marks must remain intact.

Audit possible conflicts with existing conversion modifiers before editing the grammar. Preserve existing valid conversions. If a spelling is already a conversion, distinguish the terminal Boolean type-test form in the bound AST using the existing grammar context; document and test the resolution. Do not silently reinterpret established valid queries.

Use professional internal names such as `IsNonEmpty` and `IsType`; these are examples, not a mandate for duplicate enums if current types already fit.

### 7.2 Type tests

For a selected logical value:

| Predicate | True for |
| --- | --- |
| `:string` | String values, including the empty string |
| `:number` | Numeric values, including zero |
| `:array` | Array values, including empty arrays |
| `:null` | Explicit null |
| `:object` | Object values, including empty objects |
| `:exists` | At least one selected value exists, including null |

Array, object, and null are different types. Do not use JavaScript's historical `typeof null` behavior. No string-to-number coercion or text normalization is performed merely to test a type. Booleans are not numbers.

Type tests use the logical values produced under the fixed source/schema binding, before query-specific transformations. Inferred posting types must not change their meaning. A string that inference finds date-shaped remains a string for these primitive type tests unless the existing declared logical model already converts it independently of indexing.

Map string-backed `Value::Id` consistently with existing logical string semantics. Audit other non-JSON logical variants and extension hooks; use a shared primitive-family mapping where one is defined. Do not classify an opaque extension value by guessing from its display text. An unavailable exact classification remains a capability limitation with the established evaluator/extension error behavior.

Do not add an unrequested date/ID/type-string query language in this task. Preserve any existing Boolean type-test modifier; the required new modifier list above does not require adding another spelling for it.

### 7.3 Nonempty predicate

`field:? = true` means at least one selected value is nonempty.

The empty classes are exactly: no selected value/absent, explicit null, empty string, empty array, and empty object. Numeric zero and Boolean false are **nonempty**. Whitespace-only strings are nonempty. Classification occurs before normalization, trimming, or markup removal.

A nonempty array or object counts as nonempty regardless of its children. This operator does not recursively test whether a container has meaningful descendants.

For a path resolving multiple values, nonempty dominates empty:

- Selected values `[null, ""]`: `:? = false`.
- Selected values `[null, "hello"]`: `:? = true`.
- No selected values: `:? = false`.

These examples describe the selection result, not a terminal array value. For a terminal array containing null, the array itself is nonempty. Explicit element selection tests the elements instead.

`:? = false` is the complement of the aggregate positive test, not “some selected occurrence is empty.”

### 7.4 Multivalued type tests and existing null semantics

For the new type predicates, the positive test means “at least one selected value has this type”; comparison to false negates that aggregate test. Thus absent fields satisfy a false type test. Preserve the existing `:null` and `:exists` contracts, including absent fields satisfying `:null = false`.

A multivalued selection can satisfy both `:string = true` and `:null = true`. Nonempty winning for `:?` does not erase null/type evidence for other predicates.

Keep existing selector, quantifier, positional, and correlated-element semantics. A record-level summary cannot establish that two conditions hold on the same array element. Use it only where the bound expression's scope permits exact proof.

Duplicate keys within one source object follow the format adapter's existing logical resolution policy. Do not treat overwritten lexical occurrences as logical field values just because raw/decoded FTS may retain them. Array occurrences and duplicate source keys are different cases.

## 8. Compact type, presence, and special-value representation

### 8.1 Semantic information to preserve

Use one compact field-state representation per selected canonical field/selection domain. It must preserve presence, primitive type, and special values without introducing separate permanent bitmaps for every fact.

Required occurrence classes:

| Class | Type | Nonempty |
| --- | --- | --- |
| Explicit null | Null | No |
| Boolean false | Boolean | Yes |
| Boolean true | Boolean | Yes |
| Numeric zero | Number | Yes |
| Other numeric value | Number | Yes |
| Empty string | String | No |
| Other string | String | Yes |
| Empty array | Array | No |
| Other array | Array | Yes |
| Empty object | Object | No |
| Other object | Object | Yes |

Absence means no selected occurrences in a covered record. Unsupported/unprocessed data is **not** absence. Keep coverage separate from value state.

Use the existing exact numeric equality policy for zero, including signed zero and supported numeric representations. Never convert exact integers/decimals to floating point merely to classify zero. Strings such as `"0"` remain strings.

For each record and selected path, aggregate the occurrence classes as a small set. An implementation can use an eleven-bit mask held transiently in a `u16`. OR accumulation is order-independent and preserves multiple types/special values. Nonempty is derived from membership in the nonempty classes.

This avoids the incorrect “last value wins” summary. Nonempty wins only for `:?`; the stored information still supports specific null/type tests.

### 8.2 Physical encoding

Do not store an unpacked Rust enum or heap allocation for every record-field pair. Reuse existing compact-column infrastructure where available.

A concrete acceptable baseline is:

- Four-bit packed codes for absent and the eleven individual occurrence classes.
- One mixed-state code with a compact sparse side table containing the actual occurrence mask.
- Shared record-block coverage, implicit absent/default regions, and no allocation for entirely absent blocks where the existing representation permits it.
- A bounded per-block alternative for heavily mixed data if a sparse side table becomes larger than a packed mask column.

A block dictionary of occurrence masks with packed codes is also acceptable if existing infrastructure makes it simpler. Choose one measured implementation; do not deliver several competing storage frameworks. Report bytes for real datasets and adversarial mixed arrays.

The four-bit estimate applies to ordinary single-class records before block/coverage/exception overhead. It is not a universal storage guarantee. Mixed selections require more information; do not discard it to meet a nominal bit count.

Use immutable finalized blocks and bounded mutable builders. Intern/compile field paths once; avoid path-string cloning per occurrence. Do not initialize every missing field explicitly for every record.

If a general type-only result exists but special-value classification is unsupported for an adapter, expose only the proofs actually available. Do not encode an unknown string as nonempty or an unknown container as empty.

### 8.3 Reuse and replacement

Unify existing presence/null/empty-string facts with the compact representation where it provides equivalent semantics. Adapt the common capability layer rather than adding a parallel field-state planner.

A full value posting may already answer equality to zero or empty string. Reuse it when cheaper. Avoid duplicating the same authoritative fact in permanent postings solely because both APIs exist.

Existing algorithms may require auxiliary acceleration derived from compact states. Any retained bitmap/cache must have a measured purpose, bounded ownership, and consistent lifecycle; do not recreate all state bitmaps by default.

Do not remove full length indexes: an empty-array fact answers length zero but cannot answer arbitrary lengths. Preserve configured sort/value/relation structures that hold information the compact states do not.

### 8.4 Exactness limitations

Compact states can answer type, existence, null, and nonempty predicates exactly for covered records with compatible selection semantics. They can answer special-value equality where that expression's established comparison rules match the stored fact.

They do not generally provide exact arbitrary scalar equality/ranges, array cardinality, element positions, multiplicity, same-element correlation, recursive emptiness, or transformed-value equality.

A terminal array's type/state is separate from states selected through its elements. Never classify an array of strings as a string-valued terminal field.

## 9. Partial coverage through the unified planner

For active input domain D, let C be the compatible covered domain of one capability.

For exact indexed matches M within C:

```text
must = M
may  = M union (D minus C)
```

For conservative candidates P within C:

```text
must = empty
may  = P union (D minus C)
```

Verify only unresolved records after applying other query restrictions. A partially covered index must never remove an uncovered record merely because it has no posting. Empty storage with complete coverage means no matching values; empty storage with no coverage proves nothing.

Use the predecessor's shared AND/OR/NOT bounds, cost/selectivity ordering, proven-match exclusion, and shared residual evaluation. No index-origin branches, separate inferred planner, or application-specific fallback engine.

Coverage is per capability and selection domain, not just per field or task. Values may be complete while sorting is not, or state coverage may differ from inferred dictionary coverage. Use cheap borrowed coverage references, shared full-domain markers, intervals, or existing bitmaps as appropriate; do not allocate a new collection-sized bitmap for every field unnecessarily.

Estimate index access plus residual work on the current input. Tiny covered domains do not automatically justify expensive lookup. A direct compact-state scan is a legitimate resident-index access; account for its scan cost rather than pretending it is a constant-time posting lookup.

Estimate-only EXPLAIN performs no source I/O. Extend diagnostics to show covered/uncovered counts, capability revision, selected access, exact/candidate status, and residual work without forcing expensive instrumentation on ordinary queries.

## 10. All query surfaces remain correct during construction

Cover ordinary search, field-only search, combined FTS/FILTER, count, streaming, pagination, sorted results, relation-local predicates, exports, and CLI batch queries.

- Bigrams remain candidate filters; normal text verification is still required.
- Missing field acceleration must never become an unsupported-query error for an expression that already has source evaluation.
- Fallback uses the same bound semantics and borrowed/shared evaluation paths as indexed execution.
- Exact counts include verified uncovered records. Partial index coverage alone does not make an otherwise successful query result incomplete.
- Actual source failures retain the existing incomplete-result/error contract.
- Sorting includes covered and uncovered records under one comparator and stable tie-breaker. Do not emit final top results from the indexed subset while ignoring unknown keys.
- A computed/global sort may delay its first final page; describe that honestly and preserve existing bounded execution.
- Relation resolution/backlinks must preserve existing semantics. Identify indispensable identity data in pass 1 or provide the established complete fallback. Do not return an empty relation result because its optional acceleration is pending.
- Field-based display columns and record details can fetch values lazily. Pending acceleration is not a null value.

No query automatically starts inference, waits for pass 2, or polls every posting's job status. Capture the current immutable catalogue once and plan from published capabilities.

Do not add search-time source freshness scans, modification-time checks, content hashes, format revalidation, or reconciliation. Explicit lifecycle operations remain responsible for index state. Existing necessary source I/O/error handling remains; do not claim a snapshot stores file bodies when it only stores locators.

## 11. Cache and continuation behavior

A capability-only publication must not invalidate semantically complete verified membership or ordered-result caches for unchanged data and bindings. Keep their shared memory budget; do not allocate one independent cache per publication.

Physical plan/access caches include capability identity or otherwise rebind safely. Semantic parsing/binding can remain reusable when its configuration is unchanged. An old plan must not retain a dangling pointer to replaced structures.

Pagination and streams preserve one result ordering and snapshot. Capability improvements alone must not invalidate continuation tokens, close record tabs, or clear existing results. Stable tie-breaking must make indexed and fallback ordering equivalent.

Actual data/configuration changes continue to invalidate or version results under the established lifecycle policy. Distinguish this from progress-only and capability-only events in both core and applications.

If an existing cache key uses one combined publication number, refactor its ownership sufficiently to make this distinction real. Do not repeatedly throw away warm query results throughout pass 2 and call the behavior acceptable without measurement.

## 12. Lifecycle, cancellation, and concurrent changes

### 12.1 Add and update

New or changed source records receive base preparation and required validation first. Publish their new active identities, locators, metadata, and text contributions atomically through the existing lifecycle. Their field capabilities start uncovered unless equivalent prepared contributions are already available.

Schedule their selected extended work through the same pass-2 mechanism. Updating a record must not temporarily combine its new text with old field facts.

A pass-2 batch carries the record incarnation/revision and configuration identity it processed. At publication, reject or restrict stale contributions against the in-memory lifecycle state. Do not attach an old batch to a replacement record sharing an address or filename.

Do not reject all useful progress merely because an unrelated record changed: reuse record-scoped eligibility checks and preserve valid batch contributions. Keep retry/coalescing bounded.

### 12.2 Delete and move

Deletion removes the record from current active domains immediately; background publication cannot resurrect it.

A compatible move retains stable content facts and updates routing/path-dependent metadata using the existing optimized move path. Pass-2 work must resolve the current valid routing or retry safely if its input disappeared. Do not force content rereads solely to update a path when the lifecycle already establishes compatibility.

Membership changes across collections follow existing atomicity and binding rules. A change of adapter/schema/text binding may require replacement; do not reuse incompatible compact states.

### 12.3 Pause, cancel, failure, and shutdown

Expose cancellation of extended construction independently of query cancellation. Completed batches remain published. Uncommitted buffers are discarded. The base stays searchable.

Support resuming/retrying uncovered work through existing task controls. Do not restart pass 1 or erase complete field coverage simply because pass 2 was interrupted.

Failures identify affected scopes/capabilities and leave other useful coverage intact. Report “complete” only if all requested work is complete or explicitly excluded by policy.

On shutdown, stop new admissions and quiesce publication before a final requested save. Do not require completing pass 2 to close a session. Preserve existing automatic-save/manual-save policies and cancellation behavior.

Repository replacement releases old subscriptions/tasks and rejects late events by session identity. No background worker may publish into a newly opened unrelated repository.

### 12.4 Rebuild and compaction

Rebuild prepares a replacement base while the old view remains usable, publishes the new base when ready, then performs its extended pass. Do not wait for the replacement's full field indexing before swapping if the user requested this fast-readiness workflow.

Compaction operates on existing stored contributions without source parsing. Remap compact states, coverage, IDs, and capability metadata consistently. Compaction is not a hidden third indexing pass.

## 13. Persistence and loading

Version the persisted format for compact states and explicit capability coverage as required by the existing compatibility policy.

A valid saved snapshot may contain:

- A complete searchable base.
- No extended indexes.
- Partially covered extended capabilities.
- Fully complete requested capabilities.

Persist the effective field-selection/inference policy, authoritative completed coverage, and sufficient resumable work description. Do not serialize live threads, locks, cancellation tokens, or a “running” flag that is treated as an active job after restart.

On load, expose a compatible base immediately; pending extended work is scheduled according to the saved policy and current explicit opening options. Never silently enable inference because a file-state structure exists.

Old snapshots remain usable under their true capabilities. Missing new metadata means unknown/unavailable capability, not invented absence or completeness. If migration can reuse old postings without source reads, do so; otherwise leave the capability unresolved and build it in pass 2.

Use atomic snapshot writes and existing destination safeguards. Save the selected coherent snapshot without waiting for all extended work unless the caller explicitly requests complete indexing. Coalesce automatic saves; do not rewrite the entire index on every field batch.

Distinguish saved data revision from saved capability revision so “new index work is not saved” can be reported without pretending source records changed.

Use existing resume support where possible. A crash before a batch commit may cause that bounded batch to be reprocessed, but cannot cause unfinished coverage to be trusted.

## 14. Library API and observability

Keep the core responsible for construction scheduling, publication, cancellation, and capability semantics. CLI and desktop are clients, not separate builders.

Provide or adapt conceptual operations equivalent to:

- Start/open a staged build and obtain its session/task handle.
- Obtain catalogue/listing progress.
- Await SearchReady.
- Capture a query view at current published readiness.
- Observe extended-index status.
- Explicitly request inference/extended work.
- Await extended completion when requested.
- Cancel/resume/retry extended work.
- Save the current snapshot.

Names must fit the current API. Preserve a blocking “build everything and return” convenience API for callers that need it, implemented by waiting on the same staged machinery. Do not retain two independent implementations.

Provide race-safe subscription plus status acquisition so a fast SearchReady event cannot be missed. Events carry instance/task/revision identity and typed status. Useful events include catalogue progress, base progress, SearchReady, capability publication, extended progress, and terminal task outcomes.

Expose totals only when known without a costly extra traversal. Track files, records, bytes, and selected-capability coverage distinctly. Coalesce ordinary progress to a bounded presentation rate; deliver readiness and terminal events promptly.

Status inspection must not walk every record or reconstruct every posting. Use maintained counters and immutable metadata.

## 15. CLI and TUI integration

### 15.1 Opening and execution defaults

Adapt the actual current commands and preserve established argument spelling where equivalent options exist. Add one explicit wait policy if none exists: `--index-wait=search|complete`.

| Invocation intent | Default behavior |
| --- | --- |
| Interactive CLI/TUI | Enter usable search/list interaction at SearchReady; continue extended indexing in the session |
| One-shot query | Execute at SearchReady using current coverage; do not wait for extended completion before output or ordinary exit |
| Query batch | Start at SearchReady; each independent query captures the current capability view |
| Explicit full index build or explicit inference-only command | Wait for its requested extended work and report its outcome |
| Explicit `--index-wait=complete` | Wait for requested extended completion; failure is reported rather than silently treated as success |

Preserve any more explicit existing mode selection. Help text must explain that `search` affects readiness timing, not result completeness.

Reuse `--auto-index` or the current inference option as an explicit inference request. If the CLI already has index-maintenance subcommands, add “infer” there; otherwise provide a clearly named explicit invocation using the established argument model. Do not create a second independent command family just to satisfy a proposed spelling.

An inference operation on a loaded/searchable base builds extended state without rereading for unchanged bigrams. Existing input/schema/cache path options remain authoritative.

### 15.2 Process lifetime and persistence

A one-shot process cannot promise continued background work after it exits. On normal query completion, cancel/quiesce unfinished pass 2 and preserve only the snapshots requested by existing save policy. Do not add a daemon or detach orphan workers.

For a query with `--write-index`, save a coherent current snapshot under the search-wait policy, including partial coverage and pending policy. Use `--index-wait=complete` for a fully accelerated saved result. Document this behavior clearly; never label a partial snapshot fully indexed.

A full index/inference command waits for its work. An inference command intended to persist acceleration must use an explicit or already configured save destination; do not invent a default disk write beyond current CLI rules.

Cancellation and exit codes distinguish query failure, explicitly requested build failure, and optional background acceleration failure. A successful ordinary query is not retroactively failed solely because optional acceleration failed; report that condition on the diagnostic channel.

### 15.3 Output

Keep stdout compatible with existing machine-readable query output. Progress, warnings, and readiness messages go to stderr or the established structured event stream. Disable animated terminal output when not attached to a TTY.

Do not add unsolicited progress objects to existing JSON result arrays. Optional diagnostics may report data/capability revisions, covered/uncovered work, and stage timings through existing extensible metadata.

In TUI, preserve the existing layout and query controls. Show a compact status such as “Search ready · Field indexes 42%” only when the denominator is meaningful. Unknown totals are indeterminate. Query cancellation and indexing cancellation remain separate.

Add parser, command-lifetime, stream-routing, partial-save/load, inference, and real CLI integration tests.

## 16. Desktop integration

### 16.1 Opening and listing

Preserve the current native GPUI opening workflow, explicit source/schema/cache choices, read-only source behavior, and existing navigation layout.

Attach the session when the catalogue can provide useful listing data. Show stable file IDs/path metadata without waiting for field indexes. Use the existing list/table surface; distinguish discovered files from parsed records and do not fabricate metadata-dependent cells.

During base construction, keep query editing responsive and show its progress. Execute the latest pending query once SearchReady arrives, or enable ordinary execution then. Avoid a queue of obsolete searches entered during opening.

At SearchReady, remove any blocking opening state and keep the same session attached while pass 2 continues. Do not call a generic reopen/reset on each capability publication.

Loaded compatible bases are searchable immediately under existing startup reconciliation policy. Extended-index resume must not block cached startup.

### 16.2 Status and commands

Use the current index management surface and a compact status indicator. Suggested user-facing states:

- “Preparing search…”
- “Search ready · Building field indexes…”
- “Search ready · Field indexing paused”
- “Search ready · Some field indexes could not be built”
- “Indexing complete”

Keep technical coverage, revisions, byte counts, and per-field details under the existing Details view. Do not expose posting internals in ordinary search flows.

Provide explicit “Infer field indexes” through the index management surface. It broadens state discovery to eligible fields/sub-fields only when invoked or already explicitly enabled for the session. Configured indexing remains the default when inference is off.

Use existing Cancel/Resume/Retry controls where possible. Cancelling a query does not cancel indexing. Closing the index panel does not cancel its task. Rebuilding, inference, loading, saving, and reconciliation obey a documented compatibility table and coalesce duplicate requests.

### 16.3 Results, sorting, and field presentation

Capability-only updates do not clear results, restart queries, scroll the table, steal focus, mark tabs outdated, or add history entries. New searches use improved coverage automatically.

Data changes retain the current explicit-refresh/automatic-refresh policy. Do not mix result batches from different data views.

Keep lazy detail/column loading. Unavailable acceleration is not an empty field value and must not cause UI hiding of present data. Source evaluation supplies requested fields as needed.

Preserve stable sort order across fallback and indexed execution. Do not switch the comparator or reorder an existing page when a sorting index becomes ready.

Add the new Boolean predicates to the existing filter builder/completion model: Nonempty, String, Number, Array, Null, and Object, with Boolean comparisons. Preserve the authoritative free-text query and normal syntax validation. Use standard controls, not a new query-construction interface.

### 16.4 Native validation

Exercise the actual native application, not a web mockup:

1. Open a fresh dataset with pass 2 deliberately held at a barrier.
2. Observe listing data, then SearchReady, then execute text, field-only, combined, and sorted queries.
3. Release extended batches and verify progress without result resets.
4. Invoke inference explicitly and observe nested fields becoming accelerated.
5. Cancel/retry extended work while continuing to query.
6. Save/load a partial snapshot and resume without rebuilding unchanged bigrams.
7. Switch repositories with work in flight and verify old events are ignored.

Capture the actual desktop under its established headless/native workflow, with normal and smaller windows, light/dark themes, keyboard navigation, and existing accessibility labels. Keep the current hamburger menu, fonts, field view, bookmarks/history, tabs, and continuous result loading.

No source parsing, construction wait, serialization, or exhaustive field discovery may run on the UI thread.

## 17. Correctness and race-test matrix

Use deterministic barriers and the existing reference evaluator. Do not use sleeps as the primary proof of ordering.

| Area | Required cases and assertions |
| --- | --- |
| Readiness barrier | Query/list behavior works while pass 2 is blocked; no optional field-builder call precedes base publication |
| Pass accounting | Normal required files parsed once in pass 1 and once in pass 2; no extra state-only pass; JSONL batching preserved |
| Unconfigured default | Inference off and no selected field indexes: only pass 1 runs; zero extended builder calls, zero second-pass source reads, no implicit field-state/value/length indexes; field predicates still return correct results |
| Field selection | No config/no inference; selected configured fields only; declared-but-unselected fields; explicit inference including nested fields; exclusions |
| Predicate truth table | Absent, null, empty/nonempty string/array/object, zero, negative zero, other numbers, false/true, whitespace |
| Type independence | Same type answers before/during/after inference and after save/load; numeric-looking/date-looking strings do not silently change |
| Multivalued paths | Mixed null/string, empty/nonempty values, mixed primitive/container types, missing nested keys, repeated same-class values |
| Scope | Terminal array versus elements; configured positions; same-element correlations; independent existential terms |
| Duplicate keys | Existing logical adapter resolution preserved; FTS lexical occurrence behavior unchanged |
| Partial coverage | 0%, sparse/noncontiguous intermediate coverage, 100%, complete-but-empty matches, capability-specific differences |
| Boolean composition | AND, OR, NOT, false type predicates, missing fields; no complement of an approximate candidate set |
| Query surfaces | Counts, streaming, pagination, sorting, relations, exports, CLI batches, desktop backend |
| Cache behavior | Capability publications preserve verified results; physical plans rebind; data changes invalidate correctly |
| Mutation races | Add/update/delete/compatible move during pass 2; stale batch cannot attach to replacement or resurrect deletion |
| Discovery | Late field discovered after earlier full traversal; selective earlier extraction remains uncovered |
| Failures | Read/parse/adapter failure, malformed fallback, inaccessible files, resource limits; no false coverage |
| Persistence | Base-only and partial snapshots; old formats; interrupted save; reload/resume; instance identity changes |
| Cancellation | Before SearchReady, after SearchReady, mid-batch, during save, at shutdown; bounded cleanup |
| Memory | Large sparse schemas, many mixed arrays, huge records, many JSONL records, long field paths, repeated publications |
| UI | No capability-only refresh/focus/history/scroll changes; independent progress; old-session events rejected |
| CLI | stdout stability, stderr progress, wait policy, inference intent, exit behavior, partial snapshot semantics |

Generate small randomized logical documents and compare all predicate results between source-only evaluation and every intermediate coverage snapshot. Include random publication order and mixed occurrences; this is meaningful proof testing, not a test mirroring a storage helper.

Test numeric zero with the exact supported number representations and Unicode/escaped string emptiness. Empty means decoded logical length zero, not source-byte spelling or normalized emptiness.

Keep source failures distinct from partial acceleration. Successful fallback over an incompletely indexed but readable corpus must produce the same complete results as fully indexed execution.

## 18. Performance measurements and acceptance

### 18.1 Measure readiness separately

Record at least:

- Start to first usable file listing.
- Start to SearchReady.
- Start to first completed text query and first displayed result page.
- Start to extended completion.
- Pass-1/pass-2 source reads, bytes, parse invocations, and CPU time where available.
- Peak temporary memory, retained index bytes, persisted bytes, and compact-state exception/metadata bytes.
- Query latency and throughput while pass 2 is paused, running, and complete.
- Cancellation latency, publication frequency, lock hold time, and progress/repaint frequency.

Count initial file discovery, locator extraction, validation, text normalization, bigram construction, persistence, and extended indexing separately. Do not hide pass-1 field preparation under a renamed phase.

### 18.2 Comparisons

Use the same corpus, build profile, alphabet, worker budget, schema/hints, and cache conditions. Measure:

1. The actual completed-predecessor baseline.
2. New base-only construction with extended work disabled.
3. Staged construction with configured indexes.
4. Staged construction with explicit inference/all eligible field states.
5. A loaded partial snapshot resuming extended work.
6. Foreground queries during extended work.

Include the normal Musenalm corpus, a multi-record file corpus, sparse nested fields, mixed arrays, and enough dynamic fields to expose metadata overhead.

Separate warm filesystem-cache and cold-cache experiments; do not present a warm second pass as a cold-storage guarantee. The extra pass is an accepted total-work cost, not an unmeasured speedup claim.

Use at least seven repetitions for focused timings and three for the established full query suite. Report medians and dispersion. Capture exact commits, dataset identifiers, hardware, worker limits, inference settings, and cache sizes.

Run the established complete query suite (194 cases at the last reviewed baseline, or its current superset) under ASCII/Unicode and result-cache off/warm conditions. Add the new predicates rather than removing older cases. Compare result IDs, counts, evidence, and ordering at base-only, partial, and complete readiness.

Existing structural predicates such as Q115–118/Q120 may legitimately use source fallback before their indexes are built. Once equivalent coverage is complete, preserve their earlier source-free behavior. Do not mislabel this intentional early fallback as a final-state regression or excuse a final-state loss of acceleration.

### 18.3 Gates

The deterministic gate is that SearchReady does not depend on optional field work and queries succeed before that work completes.

The performance gate is that configured/inferred field indexing is actually removed from the initial critical path. Compare SearchReady against the base-only control; investigate repeatable overhead above both 5% and 10 ms. Report real improvements against the full-build baseline without inventing a universal target.

At full coverage, recheck repeatable query regressions above both 5% and 0.5 ms, and material memory/build regressions. Foreground slowdown while pass 2 runs is a first-class result: tune the background budget or foreground admission before completion, rather than accepting UI stalls because total indexing finishes sooner.

Compact state storage must be measured against the facts it replaces and the additional type information it provides. Report total and incremental bytes; a denser semantic representation is not automatically smaller for every distribution. Fix pathological per-record allocations, repeated field-path cloning, duplicate authoritative bitmaps, and collection-sized copies.

Run repository-required formatting, tests, lint checks, and native application validation for touched code. Record unavailable checks explicitly. Do not alter query semantics, result limits, or cache policies to improve reported timings.

The integrator owns these acceptance measurements on the final combined integration branch. Record the tested commit, included subtask commits, baseline commit, and clean/dirty state. Any subsequent implementation change affecting the measurements requires rerunning the relevant comparisons. Present one integrated readiness/query/memory report, not a collection of incomparable subagent results.

## 19. Implementation sequence and reviewable commits

| Phase | Deliverable | Completion evidence |
| --- | --- | --- |
| 0 | Audit current builders/adapters/API, dependencies, field-selection rules, modifier conflicts, and baseline | Recorded execution map and benchmark baseline |
| 1 | Reference semantics and parser/binder support for required predicates | Source-only truth-table and multivalued/scope tests |
| 2 | Compact field-state builder/storage/capabilities behind existing construction | Exact parity, physical-size measurements, save/load tests |
| 3 | Base/extended separation, stable catalogue readiness, task ownership | SearchReady barrier tests and pass accounting |
| 4 | Incremental extended publication and shared partial-coverage planning | All intermediate snapshots match reference execution |
| 5 | Data/capability revision separation, caches, lifecycle mutation integration | Cache reuse, stale-batch and continuation race tests |
| 6 | Partial snapshot persistence, resume, cancellation, shutdown | Crash/interruption/old-format/lifecycle tests |
| 7 | CLI/TUI wait policies, inference action, output and process lifetime | Real command integration tests |
| 8 | Desktop progressive opening, status, inference controls, predicate builder | Native interaction evidence and backend race tests |
| 9 | Full performance validation, cleanup, documentation, completion report | Matched results, no obsolete duplicate path, published commits |

Adjust commit boundaries where dependencies require it, but keep the repository functional. Do not mark the task complete after only the backend phases.

The integrator merges each completed subtask into `integration/staged-index-construction` in dependency order and runs the corresponding integration checks. Phase 9 is performed against that combined branch, including CLI and desktop code. Deliver the pushed, tested integration branch for user review; do not merge it into master without a separate explicit user command.

Use narrow feature-gated intermediate commits if necessary. Remove temporary duplicate construction/planning routes at completion. Public blocking and staged APIs must share the same implementation.

## 20. Required documentation and delivery

Deliver `docs/ai/generated/staged_index_construction_results.md` containing:

1. Exact prerequisite, baseline, and final commits.
2. Final public/API lifecycle and CLI command mapping.
3. Pass-1/pass-2 responsibility table, including unavoidable adapter-specific costs.
4. Field-selection/inference matrix and explicit exclusions.
5. Predicate truth tables, array/duplicate-key semantics, and modifier compatibility decisions.
6. Compact encoding, actual byte counts, mixed-state handling, and replaced redundant storage.
7. Coverage publication, data/capability revision, cache, and continuation contracts.
8. Mutation, cancellation, persistence, load/resume, and shutdown behavior.
9. Before/after readiness, total construction, foreground-query, memory, and index-size measurements.
10. Automated checks, native screenshots, commands, and unavailable validation.
11. Remaining limitations expressed precisely; no claim of complete coverage where fallback remains.
12. Integration branch/worktree, merged subtask branches and commits, pushed final commit, and completed task-document location under the repository workflow.
13. Confirmation that the original master checkout was preserved and no merge into master was performed; the integration branch remains ready for the user's explicit merge command.

Update library/API documentation, CLI help, query-language reference, and desktop help where applicable. Explain that “search ready” means queries can execute correctly; it does not promise every field predicate has an index or every source read succeeds.

Completion requires both passes, early listing/search availability, exact queries throughout partial readiness, the requested compact representation and predicates, explicit field-selection/inference behavior, lifecycle/persistence correctness, and working CLI and desktop integration.

## 21. Reference specifications

- [Unified query planner and index capability model](https://chatgpt.com/library?file_id=libfile_efd5590d4ec081919c26ba4579793f97&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Predicate ordering and shared evaluation](https://chatgpt.com/library?file_id=libfile_4827c6d6f50c819196b6d8cdee8d3ee9&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Index lifecycle repairs](https://chatgpt.com/library?file_id=libfile_143a8361563c81919ab06733d01acc31&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)
- [Desktop lifecycle integration](https://chatgpt.com/library?file_id=libfile_b022e548a8bc81918b0615ef45bb038e&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2)



## Implementation report

Task identifier: `staged-index-construction`. Completed: 12 November 2026.
Base and completed prerequisite commit: `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`.
Unified planner completion: `a68cb0c67b68987251d35f528ccbbe3445509ea1`.
Integration branch: `integration/staged-index-construction`.
Integration worktree: `/home/simon/source/objekte-integration-staged-index-construction`.
The original checkout remained on `master`; this task has not been merged into it.

### Delivered behavior

Core construction now publishes a file catalogue during discovery and a validated, searchable base before requested optional field work. The second source pass builds configured or explicitly inferred field capabilities in bounded batches with exact coverage, immutable query views, capability revisions, pause/resume/cancel, and partial snapshot restore. An unconfigured, inference-disabled repository skips that pass. The normal parser, binder, evaluator, planner, CLI/TUI, and native desktop support `:?`, `:string`, `:number`, `:array`, `:null`, and `:object` while preserving `:exists`. Field selection and fallback semantics follow this specification. The blocking build API uses the staged implementation.

Contributing subtask branches were `task/staged-index-construction/field-states` (`0ada21b8`), `task/staged-index-construction/construction` (merged checkpoint `b91bca5c`), `task/staged-index-construction/cli` (`b222c7e4`), and `task/staged-index-construction/desktop` (`8901f688`). Integration fixes include computed-sort cache (`01d3fafa`), staged proof (`df3571fd`, `c94cedf4`), active bitmap work (`559b49ee`), foreground worker admission (`fdff7185`), and catalogue publication (`21265636`). The cancellation timer was committed as `26e090ff`; the full verification bundle and expanded documentation as `278a2c94`. All implementation commits and merges are pushed on the integration branch.

### Validation and measurements

At the measured combined implementation revision `21265636`, `cargo test --workspace --locked` passed: 850 tests, 0 failed, 17 ignored across 22 groups. `cargo clippy --workspace --all-targets --locked` passed with existing unused-code warnings. The implementation and verification-report diff passed `git diff --check`; this preserved task specification has original Markdown hard-break spaces that Git flags as trailing whitespace. The repository-wide spacing formatter check still fails on an unchanged `storage.test.rs` blank-line convention, so unrelated formatter rewrites were discarded. A native Sway desktop run covered Open, live catalogue, SearchReady query/filter, explicit inference, Details, narrow layout, and keyboard preview. Evidence is in [the native validation report](../generated/staged_index_construction_desktop_validation.md).

The final combined benchmark used the Musenalm corpus fingerprint `259b435df73c0eb2d1d4f066f1528f9dc63c14b29766d19798078fdcd569b473`, four query workers, one default extended worker, ASCII/Unicode, verified-result cache off/64 MiB warm, and three repetitions of the complete suite. Run `target/release/examples/bench_staged_musenalm --mode base|partial|complete [--inference] --snapshot --repetitions 3 --result-dir target/<mode>` with `--snapshot-resume` for partial, then `python3 bench/compare_staged_results.py bench/results/staged-index-construction/baseline-suite target/<mode> --output target/<mode>/comparison.json`. The actual commands are recorded in the archived manifests. Across 202 cases and four profiles at each of four readiness modes, all 2,424 samples per mode succeeded; comparison with the 194-case predecessor found zero result, count, evidence, or order mismatches. The eight new predicates passed at every readiness mode. Seven paired warm trials measured 2.133 s median SearchReady versus 2.552 s median predecessor full build; advisory data-page eviction yielded 2.272 s versus 2.705 s. Configured completion took 6.644 s median and explicit inference completion 11.204 s. A seven-repeat sparse/mixed-field fixture and seven active-build cancellation trials also passed; `Index::quiesce()` took 35.47 ms median while pass 2 was running. The full setup, distributions, source-read counts, persisted sizes, query regressions, and archived raw files are in [the results report](../generated/staged_index_construction_results.md) and [benchmark evidence](../../../bench/results/staged-index-construction/).

### Limits and follow-up evidence

The extra source pass is intentional. Configured complete snapshots and process RSS are materially larger than the predecessor (165.40 MB versus 129.14 MB snapshot; 928.9 MB versus 533.8 MB measured ASCII process RSS); explicit inference is larger again. See [the footprint issue](../issues/perf/staged_extended_snapshot_and_memory_footprint.md). Some structural queries on `indexed: false` paths lose the predecessor's implicit acceleration under the required exact-selection rule, although explicit inference restores source-free execution. See [the unselected-field issue](../issues/perf/staged_unselected_structural_field_fallback.md). During optional work, focused foreground queries remained 13–21% slower than the paused base on the same host. Publication count and quiesce duration were measured; lock-hold time, per-batch peak temporary memory, and native repaint rate were not instrumented, and advisory eviction is not proof of physical cold reads. These are recorded limitations, not correctness failures or authorization to implement separate issues.

The integration branch and worktree are retained for review and an explicit merge command. After the first completion commit was pushed, the clean, merged CLI, construction, and desktop subtask worktrees and the detached baseline worktree were removed with normal `git worktree remove`. The unrelated borrowed-segment integration worktree was left untouched.
