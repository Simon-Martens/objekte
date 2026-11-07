# Objekte optional indexing, broader sorting and streaming implementation specification

## 1. Assignment

Implement four extensions to the current format-independent search core:

1. Conservative shared-bigram candidate restriction for FILTER predicates on undeclared fields.
2. Explicitly opt-in field inference and automatic typed postings.
3. Sorting by unregistered fields, positional paths, declared relations and explicit array reductions, plus optional composite sort indexes.
4. Progressive query delivery with ordered batches, provisional page replacement, cancellation and bounded buffering.

Execute this assignment through one supervisor, independent implementation workers in separate Git worktrees, and bounded research/test/review subagents. Complete worker implementation and review before the final merge phase. The supervisor owns integration, cross-feature validation, final measurements and promotion to the development branch.

The primary product behavior is a schema-free collection: the caller supplies its name, directory and reader; the core indexes the reader's ordinary text without requiring field declarations or constructing typed field dictionaries. Inference is an optional accelerator. Sorting and streaming must work with inference disabled.

This is an implementation specification, not a request to run these agents while preparing the document. When given to an execution agent, it is the complete work order.

### 1.1 Required scope and priorities

| Priority | Required result |
| --- | --- |
| P0 | Default collection opening remains schema-free, inference-free and free of typed field postings unless explicitly requested. |
| P0 | New candidate restrictions never omit a true result or suppress an observable evaluation error. |
| P0 | Inference, indexes, collected execution and streaming preserve identical query membership. |
| P0 | Sorting has a deterministic total order, bounded retained state and explicit behavior for arrays, missing values and related records. |
| P0 | Supervisor completes the final merging and integration task; the deliverable must not stop at four unmerged feature branches. |
| P1 | Selective undeclared text predicates demonstrably avoid source parsing through bigram restrictions. |
| P1 | Inference collects observations during the existing construction pass when enabled. |
| P1 | Registered ordering keeps its existing fast path; unregistered sorting uses bounded key selection. |
| P1 | Streaming emits useful events before execution finishes where its delivery mode permits. |
| P1 | Documentation, CLI examples, deterministic correctness evidence and reproducible performance reports accompany the implementation. |

Speed and retained memory are acceptance criteria. Implement bounded, measurable algorithms before adding elaborate policy machinery. Do not trade a whole extra parse pass, a document-sized clone per match, or an unbounded intermediate collection for easier integration.

### 1.2 Explicit exclusions

Do not implement runtime source updates, filesystem watchers, arbitrary graph expansion, automatic joined payload expansion, SQL, arbitrary user expression evaluation, a new asynchronous runtime, automatic schema-file modification, new production markup renderers, automatic relation declaration, or a general background indexing service.

Do not silently broaden the logical model to arbitrary JSON in this assignment. The current reader/model restrictions, including object records and homogeneous arrays, must be documented in the schema-free example. A malformed or unsupported record continues to follow the existing diagnosed fallback contract.

Do not add extra source-change-during-read checks or per-candidate metadata reconciliation. Use the established source-access contract. The user does not require protection against concurrent source modification during initial index construction.

Full compatibility with the archived library API is not required. Preserve current successful APIs and behavior unless a change is explicitly specified below. Where request structures must change, update workspace callers and document the migration.

This assignment supersedes the earlier query-extension specification's registered-column-only sorting restriction. It also authorizes the bounded schema-free construction helper and explicit inference modes described here. Other established query, relation, fallback and source-access semantics remain in force unless this specification states an exact replacement.

## 2. Baseline and prerequisite verification

Repository: Simon-Martens/objekte.

The GitHub inspection on 2026-11-07 observed:

| Reference | Observed commit | Meaning |
| --- | --- | --- |
| development/local-file-search-core | e21be07414ca4765ffa198032090c30de8683402 | Current development baseline inspected for this specification. |
| integration/typed-query-extensions | 4609e5243e60735733ce2c3ed142765359fdcfea | Ancestor of development, twelve commits behind in the observed comparison. Do not restart from it. |
| experiment/schema-independent-field-indexing | 02163b3271c041f7f79ea3cd275a70244a7a8671 | Existing experiment to inspect for useful evidence; not accepted implementation merely because it exists. |
| master | 5e3ceab56846c8bc57473a3fd9a4377f817521b6 | Original implementation, useful for behavior and test ideas. Not the implementation base. |

These are observations, not permanent moving-ref selections. Before creating worktrees, the supervisor must fetch, inspect the current development ancestry, inspect worktree cleanliness, read repository instructions and select a fixed validated commit. Incorporate already completed related fixes rather than recreating or reverting them.

Read at minimum:

- Repository AGENTS.md files, if present.
- docs/development/code-guidelines.md.
- docs/development/query-extensions-ledger.md and the final reports it names.
- Current query, source, relation, sorting, storage and cache implementations.
- Existing experimental inference branch changes and measurements.
- Relevant archived tests for inference, sorting and streaming.

The existing ledger records a completed query-extension validation and benchmark run, but its measured code predates later development commits. Match revision, corpus, configuration and timing boundaries before reusing it as a performance baseline.

### 2.1 Verified starting behavior

| Area | Current behavior and implementation consequence |
| --- | --- |
| Undeclared fields | Schema::field_or_dynamic binds Unknown, without typed postings. Keep this dynamic contract. |
| Shared bigrams | Built-in JSON text extraction includes nested string values without field declarations. Candidate membership is record-level, not a separate index for every field. |
| Current FILTER optimization | executor.rs requires a string kind and a safe scalar/ANY shape before offering the raw bigram path. Removing just the type check is insufficient. |
| Empty selections | Present empty arrays can satisfy a default universal comparison without any string witness. Missing paths do not. |
| Discovery | Recursive discovery exists, but its current default is false. |
| Identity | Filename identity uses only the filename stem. Recursive duplicate basenames therefore collide. |
| Inference | There is no accepted general inference workflow in the inspected development core. An experiment exists separately. |
| Sorting | Requests assume registered scalar columns; current ranking/cache paths rely on those columns. Related sorts are explicitly rejected. |
| Mixed sorting types | Existing scalar comparison treats unlike value variants as equal in places where registered homogeneous columns made that assumption safe. That is not a valid general dynamic comparator. |
| Default order | Collection name and record identity determine the current default order. Do not reintroduce the archived modification-time ordering. |
| Streaming | search returns a completed SearchResult. Calling callbacks after this return is not progressive execution. |
| Ownership | Open repository data, record slots and relation generations are immutable during a query. Preserve that property. |

### 2.2 Responsibility map

| Responsibility | Existing area / proposed cohesive extension |
| --- | --- |
| Dynamic binding and truth semantics | reference.rs, query_path.rs |
| Candidate paths and cost estimates | executor.rs |
| Shared structural facts and inference observation | build.rs, source.rs, narrow construction-observer helpers |
| Inference reports and bounded builders | New inference.rs and focused submodules |
| Inferred posting admission and coverage | posting.rs, posting_values.rs, inference modules |
| Sort expression parsing and normalization | query.rs, query_path.rs, workload.rs |
| Sort binding, keys, reducers and composite orders | sorting.rs and new sort_binding / sort_execution modules as justified |
| Relation key extraction and target reuse | relation_query.rs and shared grouped source-access facilities |
| Common ranking, execution and delivery | run.rs, new streaming.rs |
| Public options, generation ownership and validation | repository.rs |
| Persistence and format compatibility | storage.rs |
| Membership and ordered-result reuse | prepared.rs, verified.rs |
| Schema-free demonstration | New crates/core/examples/musenalm_without_schema.rs and small shared folder-definition helper |

Do not put every new branch into run.rs or reference.rs. Extract a cohesive module when responsibility is clear; avoid a broad unrelated reorganization.

## 3. Default behavior and shared contracts

### 3.1 Schema-free collections remain the default

A folder collection must be usable with:

- A caller-supplied collection name and relative directory.
- The built-in JSON reader, or an explicitly registered reader.
- No field declarations: an empty Schema.
- No declared relations, custom field indexes or sort columns.
- Inference disabled.
- Ordinary shared bigrams and the existing inventory/identity structures.
- Recursive discovery enabled by the folder helper.

“No typed indexes” means no per-field typed lookup dictionaries or inferred sort columns. It does not prohibit identity lookup, record inventory, shared text postings, or one small structural bitmap needed for sound planning.

Do not load .objekte collection schemas implicitly in this helper or example. Do not enable inference because a file looks like a schema, because a schema happens to be nearby, or because a query mentions an unknown field.

Provide a small general folder-definition constructor and the Musenalm example using explicit collection names/directories. Use actual directory paths from the fixture at execution time. The example must accept equivalent user-supplied folder definitions rather than putting Musenalm field names inside the engine.

Recursive admission still uses reader-specific file selection. A JSON collection admits JSON files; “schema-free” does not mean that arbitrary binary files can be parsed as JSON. Document hidden-file and symlink policies.

For recursive collections, supply a stable relative-file-path identity option or an equivalent reader identity that needs no logical field name. Preserve the existing Filename policy for compatibility. Relative-path identity must use the repository's reversible native-path representation, include directory components, define extension handling and reject actual duplicate identities. It must distinguish a/item.json from b/item.json. JSONL requires a distinct record identity policy; do not give all lines in one file the same path ID.

### 3.2 Separate semantic declarations from physical access paths

Maintain distinct concepts:

1. Declared fields: validation, conversion, relation and explicit operation contracts.
2. Observed summaries: descriptive facts from one source generation.
3. Accelerators: optional indexes with explicit domain and coverage.
4. Bound query expressions: actual parameter values, path semantics, quantifiers and operations.
5. Delivery options: how an execution exposes results.

An observation must not become a validation rule. An accelerator must not change logical types, comparison rules, accepted records or query errors. A delivery mode must not change membership.

Prepared templates, physical decisions and verified answers must retain the correct distinctions. Bind parameters, time macros, regex and relation paths once per execution. Do not read the clock again for every stream event or sort batch.

### 3.3 Shared interface checkpoint

Before workers start, the supervisor publishes a compiling contract checkpoint containing the smallest useful agreed interfaces:

- Inference mode/policy/report ownership and configuration identity.
- A narrow construction hook for structural facts and optional inference.
- Sort expression, reducer, bound key source and comparison contracts.
- Execution delivery sink, cancellation token, progress and terminal outcome contracts.
- Cache identity responsibilities and the owner of each shared integration point.
- Public request compatibility strategy.
- Resource accounting fields and error categories.

Use current interfaces where possible. Do not create empty abstractions for hypothetical future services. An interface checkpoint may contain narrow adapters and tests; it must not hide unfinished feature behavior behind public no-op implementations.

## 4. Undeclared FILTER bigram restriction

### 4.1 Required behavior

Positive text comparisons on undeclared fields must be eligible for conservative restrictions using the ordinary shared bigram index when a true nonempty evaluation requires a string witness in the indexed representation.

Examples:

~~~text
title ~ 'Goethe'
title = 'Goethe'
title:lower ~ 'goethe'
contributors.name ?~ 'Goethe'
items[*].title ?= $title
~~~

Eligibility comes from the actual bound string operand, the operator, selector semantics and representation coverage. Keep the field kind Unknown. A string comparison does not prove that all records store a string at that path.

Continue verifying the precise field, path, quantifier, case behavior and full literal. Whole-record grams can come from another field, another array occurrence or the wrong order of characters.

Do not require inference, typed postings, per-field field lists or a schema file for this optimization.

### 4.2 The universal empty-selection problem

At the inspected baseline:

~~~text
{"title": []}
~~~

can satisfy a default universal positive comparison on title even though there is no string containing the requested grams.

An empty selection can also arise through an empty ancestor array. By contrast, items=[{}] with the path items.title is missing, not a present empty selection, and does not satisfy that comparison.

Preserve these existing semantics. Do not “fix” the optimization by changing ALL to ANY, declaring unknown paths scalar, or dropping empty arrays.

Required baseline solution:

- Build one collection-level bitmap of logical records containing an empty array at any depth.
- Collect this fact during the ordinary extraction/value traversal. Do not reparse records.
- Treat it as a conservative superset of records whose universal predicate might succeed without a string witness.
- Keep complete structural coverage information. A sample is not complete coverage.
- Keep the bitmap separate from inferred field indexes; it exists when inference is disabled.

For current input domain I, shared gram candidates G and empty-array superset E:

~~~text
Positive ANY:          I ∩ G
Positive default/ALL:  I ∩ (G ∪ E)
~~~

Every result still requires exact predicate verification unless a separate existing proof establishes it. E proves neither field presence nor predicate truth.

The supervisor and semantic review subagent must verify the selector proof against the pinned code, including explicit wildcard and index selectors. If a selector or reader can produce a successful empty selection outside E, either extend the conservative structural summary or decline this restriction for that case. Never assume completeness from the built-in JSON examples alone.

For a selected explicit scalar position, omit E only when its semantics prove that no empty comparison can succeed. An out-of-range position remains missing.

### 4.3 Eligibility and fallback

Preserve all existing representation and reader coverage checks:

- Logical strings must be covered intact by the selected raw/shared representation.
- Case-folded matching uses compatible folded grams and the established preparation routine.
- Unsupported alphabet fragments, too-short literals and patterns with no useful grams scan.
- A custom reader that omits strings cannot lend its shared index to arbitrary field predicates.
- A custom matcher must explicitly permit candidate skipping and representation compatibility. Do not suppress errors from a fallible operation by avoiding its invocation.
- Raw fallback text is not evidence for logical field values.
- Scalar field-to-field predicates and source-dependent relation predicates do not automatically qualify.
- Related predicates remain under the existing relation planner and proof contracts.
- Negative operators and NOT must not complement approximate gram sets.
- LIKE patterns containing wildcard or escape syntax use only already-proven necessary-literal extraction; retaining a scanner is acceptable. A new LIKE optimizer is not required.

Boolean composition must preserve three-valued unavailable semantics. In particular, metadata OR branches may select fallback records; a logical candidate restriction must not remove them globally.

Cost the actual union with E. If empty arrays make it broad, scanning may be cheaper. Expose the reason in EXPLAIN rather than forcing the index.

### 4.4 Storage, counters and evidence

Persist and validate the structural bitmap with its record-slot domain and reader/extraction semantics. Shift local slots correctly when merging parallel build batches. Validate restored coverage and reject incompatible snapshots.

EXPLAIN/ANALYZE must identify:

- Undeclared dynamic field binding.
- Positive string-witness restriction.
- Representation and coverage decision.
- Whether empty-selection preservation was required.
- Gram candidates, structural candidates and final restricted-domain size.
- Retained residual and any fallback reason.

Use aggregate counters; do not allocate a diagnostic object for every candidate.

### 4.5 Acceptance

Add independently specified expected results and candidate-superset tests covering:

- Same path scalar, array, number, Boolean, null or absent across different records.
- Empty strings; terminal, ancestor and unrelated empty arrays.
- Nonempty arrays of objects with missing child fields.
- Universal and ANY equality/contains; arrays containing one false witness.
- AND, OR, NOT, metadata OR, raw fallback and unavailable branches.
- Positional selectors and explicit wildcards.
- Parameter changes; case folding; Unicode; both bigram alphabets.
- Short literals and unsupported wildcard/escape cases.
- Complete and partial custom reader coverage.
- Fresh build, parallel batch merge, restore and disabled/warm verified cache.

For at least one selective undeclared ordinary FILTER, deterministic counters must prove fewer source reads/parses than forced scanning with inference disabled. Also include a broad empty-array workload where declining the optimization is reasonable.

## 5. Opt-in field inference and automatic indexing

### 5.1 Modes and activation

Expose three explicit modes in the Rust opening/build configuration and command-line demonstration:

| Mode | Required work |
| --- | --- |
| Disabled | Default. No inference path catalogue, field observation counters or provisional typed indexes. |
| ReportOnly | Observe fields and return/report summaries; do not publish inferred typed postings. |
| AutoIndex | Observe fields and publish admissible complete Boolean, number and low-cardinality string postings. |

ReportOnly must not build and discard every typed index just to produce its report. It may retain bounded distinct-value statistics needed for admission advice.

Required activation is at repository construction/opening. A user may reopen with a different mode to enable it later; clearly report when that requires rebuilding. Do not require a live mutable-index publication framework for this assignment. An explicit post-open report scan may reuse the same collector, but cheap live index activation is not an acceptance requirement.

No hidden background task starts on ordinary open or first query. No source or schema file is modified. Schema writing and automatic relationship/text-format guessing are outside this implementation.

At construction, enabled inference must share the already parsed logical record and existing traversal. It must not independently reopen or parse every file. Keep default build time free of enabled-mode bookkeeping.

### 5.2 Report contract

Report per collection and canonical logical path:

- Observed scalar kinds and structural shapes.
- Scalar versus array occurrence, array nesting and compatible element kinds.
- Integer-only versus fractional numeric observations, without inventing a new stored numeric semantics.
- Eligible logical-record count and unavailable/fallback-record count.
- Present-record count, records-with-null-occurrence count and empty-array-record count.
- Scalar/value occurrence count separately from distinct-record membership.
- Observed missing count over the eligible domain.
- Bounded distinct string summary and low-cardinality admission result.
- Index built, not requested, redundant, disabled, incompatible, incomplete or budget-exceeded status.
- Reasons, completeness and retained byte estimates.

Define counting precisely. Presence means the path exists under the same selectors as query evaluation. A repeated value in one array contributes one posting membership for that record. A path can have both null and non-null occurrences in the same record; report counts must not pretend these sets are disjoint. Empty ancestor traversal and explicit terminal null need separate treatment.

The report's records-with-null-occurrence statistic is not the exact :null predicate set. Under the existing evaluator, :null is true only when the pre-flatten selection is exactly one explicit null. A selection containing null and a string is not :null, and a terminal array containing null is still an array. Derive exact selector-state postings separately or retain a residual; never use the overlapping observation statistic as the existing null bitmap.

Compute missing records from the complete eligible logical domain and the presence set. Fallback records are unavailable, not missing. Do not count a malformed record as evidence that every field is absent.

Value-only observation needs an additional structural reconciliation step. If one record has items=[] and a later record introduces items.title, the current selector regards items.title in the first record as a present empty selection. Retain bounded ancestor empty-selection facts and propagate them into discovered descendant presence/empty posting state when finalizing reports and indexes. This must work when the descendant path is first discovered in a later parallel batch. Do not count only encountered terminal leaves or incorrectly mark the earlier record missing. Use path/ancestor bitmap propagation or an equivalently sound bounded method, without an all-fields-by-all-records reparse.

Empty arrays and all-null observations do not establish a non-null element type. Across-record type or scalar/array shape disagreements produce a mixed summary. Initially decline typed index admission for incompatible mixed shapes instead of inventing a complicated partial type system.

Do not convert numeric-looking strings, timestamp-looking strings or ID-looking strings. Do not infer a relation because strings happen to match target IDs. Objects and arrays of objects are observed structurally; their eligible leaf paths may be indexed independently.

### 5.3 Admission policy

AutoIndex can admit:

- Boolean values.
- Existing exact numeric values using the current Number/ScalarKey semantics.
- Low-cardinality strings with a bounded dictionary.
- Associated presence/null/array facts needed by the existing typed posting semantics.

It must not create sort columns or composite orders automatically. Sorting acceleration is a separate explicit option.

Declared field contracts remain authoritative. Avoid duplicate dictionaries for an already compatible explicit posting. Respect explicit opt-outs. Because the current indexed=false representation may not distinguish an explicit prohibition from an unspecified default, define that distinction deliberately in the inference policy or registration metadata. Do not silently treat every undeclared path as prohibited, or every explicit false as permission.

Initial low-cardinality string heuristics:

- At least 20 nonblank occurrences from at least 10 records.
- No more than 64 distinct nonblank strings.
- Each admitted string at most 64 UTF-8 bytes; at most 8 KiB retained distinct text.
- Distinct count at most max(4, floor(nonblank occurrences / 10)).
- At least 80% repeated nonblank occurrences.

Expose these thresholds as versioned configurable admission policy. They are heuristics for whether an index is useful, never closed-enum validation. Blank strings do not qualify a field as low-cardinality by themselves, but an admitted posting must still represent blank strings, nulls and presence correctly.

If a long value, excess distinct count, mixed kind or memory limit makes a provisional string index unsuitable, discard it permanently for that scan and report why. Do not repeatedly rebuild it when later values look favorable.

### 5.4 Bounded construction and parallel merge

Implement observation by visiting values, not by checking every discovered path against every record.

- Intern canonical paths using typed path steps. Object key "0" is not array index 0, and punctuation in keys must remain unambiguous.
- Aggregate repeated occurrences without a large per-record hash allocation when existing scratch can be reused.
- Reuse existing posting builders where coverage and semantics agree.
- Bound path count, path depth, per-field values, per-record contributions, retained distinct text and total inference bytes.
- Include worker-local state, buffered contributions and merge-time state in the budget. A per-worker 64 MiB bound is not a repository-wide 64 MiB bound.
- Record chosen numerical defaults in the shared contract before coding; use existing repository limits where they already fit.
- Use deterministic admission/drop decisions so worker count does not unpredictably choose different field indexes.
- On an exceeded optional-index budget, drop the affected accelerator and preserve exact query fallback. Do not drop records or silently truncate posting memberships.
- Reports may identify omitted paths/counts due to a reporting budget. Mark incomplete summaries explicitly.
- An incomplete sampled observation cannot authorize a complete exact posting or a NOT complement.

The initial implementation should prefer complete indexes or no index. Partial indexes are permitted only if the existing coverage model retains a conservative remainder and tests establish that every operator remains correct. They are not required for acceptance.

### 5.5 Planner integration without semantic rebinding

Query binding stays based on explicit declarations and actual operands. Observed field kinds must not make the same query begin rejecting literals or changing missing/array behavior.

The planner may consult inferred accelerators and complete coverage facts to choose a typed path. It must preserve the ordinary dynamic evaluator as the semantic oracle.

This may require an access-path adapter between a dynamically bound term and a homogeneous observed posting. Verify:

- Different parameter types across executions.
- Numeric equality and ranges without string coercion.
- ANY/default ALL and empty arrays.
- Missing/null behavior and negative operators.
- Present mixed values when admission was declined.
- Unsupported transforms and field operands.
- Fallback records and mixed logical/fallback domains.

An exact posting proof requires a complete compatible domain, not merely a persuasive sample. Keep exactness and selectivity separate.

### 5.6 Snapshot and cache rules

Inference mode, policy version, thresholds, extraction version, explicit index policy and retained coverage participate in compatibility.

Disabled opening must not silently restore an AutoIndex snapshot and enable inference indexes. It may reuse compatible base sections if the storage design already supports that safely; otherwise a clearly recorded rebuild is acceptable. Preserve the established source reconciliation contract.

Serialize summaries and admitted indexes only when enabled and useful. Validate path identities, record-slot bounds, counts, presence/null sets and posting coverage before publication.

Prepared physical choices must not outlive the index capability/configuration they were selected against. Clearing repository-local caches on a changed opening configuration is acceptable. Inference must not change verified membership identity except where repository-instance/configuration ownership already separates it.

### 5.7 Acceptance

For the same source data and query corpus:

~~~text
Disabled results == ReportOnly results == AutoIndex results
~~~

Compare complete IDs, counts, order, evidence, payloads and observable errors. Repeat fresh/restored and cache-disabled/warm.

Test nested objects/arrays, empty/all-null fields, repeated array values, scalar/array changes across records, mixed types across records, large integers, fractional numbers, blank strings, high-cardinality strings, many dynamic keys, budget exhaustion, explicit opt-outs, existing indexes, custom readers and fallback records.

Include descendant paths discovered only after empty ancestors were observed. Reverse record order and change parallel batch boundaries; presence, empty-selection counts, :exists results and universal comparisons must stay identical. If structural observation limits prevent complete reconstruction, mark the affected summary incomplete and decline exact inferred posting admission.

Require:

- Disabled mode has no inference-owned field maps or inferred postings.
- ReportOnly has no published inferred posting.
- AutoIndex accelerates a selective Boolean/numeric/low-cardinality query without reading every source.
- Enabled build performs no extra parse of each record.
- Dropped indexes retain exact fallback answers.
- Worker-count changes preserve reports and admitted indexes under the same deterministic policy.
- Restore preserves the same policy and observations.

A late reopen/rebuild with inference enabled must be measured separately from a first construction with inference enabled. Do not present those as equal-cost operations.

## 6. Broader sorting

### 6.1 Required expressions and request compatibility

Support sorting with inference disabled by:

- Undeclared local scalar fields and nested object paths.
- Existing metadata fields without prior sort-column registration.
- Explicit array positions.
- MIN, MAX, FIRST and LAST reductions over multivalued paths.
- Finite forward relations and registered backlinks.
- Compatible registered scalar transformations or text representations.
- Multiple keys with independent directions.
- Sort-only requests, FILTER plus sort, and combined FTS/FILTER plus sort.

Preserve the current inline versus independent-option conflict rule. A request containing both nonempty forms fails clearly rather than silently combining them.

Normalize inline and programmatic sorting into one typed specification. Use the existing QueryPath parser and relation binder; do not introduce a second field-path language.

Required inline grammar:

~~~text
sort_clause := SORT BY sort_item ("," sort_item)*
sort_item   := sort_expression [ASC | DESC]
sort_expression := field_expression
                 | MIN "(" field_expression ")"
                 | MAX "(" field_expression ")"
                 | FIRST "(" field_expression ")"
                 | LAST "(" field_expression ")"
~~~

Recognize these keywords only in their sort grammar positions, case-insensitively. They remain usable in quoted data keys. Preserve registered field modifiers in field_expression.

Examples:

~~~text
SORT BY title
year >= $minimum SORT BY publication.date DESC, title ASC
SORT BY contributors[0].name
SORT BY MIN(prices[*]), title
SORT BY MAX(contributors.organization.name)
SORT BY entry.preferred_title, @record.id
SORT BY MIN(@backlinks.mentions.year)
SORT BY body:normalized
~~~

The final example requires an existing registered compatible operation named normalized; it does not imply a newly built-in renderer. Reducers operate within each source record's reached values, not across the result set.

Support an equivalent programmatic expression/reducer structure. Retain existing simple SortField requests through a documented conversion or backward-compatible defaults. Do not encode hidden reducer semantics only inside an opaque field string.

### 6.2 Bound sort architecture

Bind each normalized term to one of:

1. A compatible existing resident scalar column.
2. A compatible configured composite order.
3. A computed scalar key from local logical data, metadata or a finite related path.

One common key comparator must serve stored indexes, computed top-k selection, cached membership ordering, the reference implementation and streaming ranking. The independent oracle must still implement expected ordering separately.

Prepare path resolution, relation hops, operation handles and ICU configuration once. Comparators must not read files, parse data, resolve registries, transform text or allocate ICU sort keys.

An index is an access path, not a requirement for sort validity. Enable dynamic sorting by default. An explicit caller option such as RequireResidentOrder may reject computed sorts for latency-sensitive applications; do not make a confirmation prompt or that restriction the ordinary behavior.

Known unsupported declarations fail during preparation. Runtime sort-expression errors on matching records must remain observable; do not hide an invalid object or unreduced array simply because it would not fit the current top-k page.

### 6.3 Total ordering

Define and test a total order before introducing dynamic keys.

For different present scalar kinds, use this stable ascending rank:

~~~text
Bool < Number < Date < Datetime < Unixtime < Id < String
~~~

Within each kind, use existing typed comparison semantics:

- Numbers retain exact current integer/fraction comparisons; do not stringify or narrow integers.
- IDs use exact canonical ordering, without case folding.
- Strings use the bound ICU collation.
- Dates, instants and Unix-second values remain distinct types.
- Numeric-looking or date-looking strings remain strings.

Descending reverses comparisons among present scalar values, including the kind rank. It does not reverse identity tie-breakers or independently configured null/missing placement.

Null, missing and unavailable remain distinguishable during extraction. Preserve current configured null/missing ordering. Unavailable uses the missing placement bucket for ordering, while retaining its diagnostic classification; identity breaks otherwise equal exceptional keys. Do not invent a false scalar value for a dangling relation or a fallback record.

For an undeclared path, preparation must permit runtime singularity checking: Unknown is not proof that the value is an array. A reached scalar without array expansion is sortable, so schema-free SORT BY title works without any declared type. Track whether traversal actually expanded an array, rather than relying only on the current conservative multi flag. An actual unreduced array or array-expanded multivalued path is an error even if only one value remains; an explicit position or reduction establishes the intended scalar. Missing paths and present empty selections produce Missing. Ordinary objects are not sortable. Do not silently take the first value, serialize an object to JSON, or compare unlike scalar kinds as equal.

Equal user keys use the existing deterministic collection/record identity tie-break. Preserve current global cross-collection ordering.

Test antisymmetry, transitivity and equality consistency across all supported kinds and exceptional states. The previous unlike-variant-equals behavior must not survive in the dynamic comparator.

### 6.4 Reduction contract

For MIN/MAX, apply the registered transformation to every relevant terminal value before comparison. For FIRST/LAST, choose the occurrence structurally, then transform only the selected value. Do not evaluate a fallible transformation on unselected occurrences in FIRST/LAST; their transformation errors are outside that expression's semantics. Path traversal and unavailable-occurrence rules still determine whether the selected occurrence can be established.

For MIN and MAX:

- Compare present valid scalar values using ascending value order, regardless of final ASC/DESC.
- Ignore null and missing occurrences when at least one valid scalar exists.
- Return Null when there are null occurrences but no valid scalar.
- Return Missing when no values/nulls are reached, including an empty array.
- Return Unavailable if an unresolved or unavailable occurrence could change the reduction. The initial conservative implementation may return Unavailable whenever any reached occurrence is unavailable.
- Reject object-valued or otherwise unsupported terminal occurrences instead of dropping them.

For FIRST and LAST:

- Use the actual source occurrence order, including duplicate references and nested array order.
- Preserve explicit null as a value at its occurrence position.
- No reached occurrence yields Missing.
- Do not use deduplicated target order, target IDs, hash iteration or posting order.
- If an unavailable occurrence prevents establishing the selected first/last value, return Unavailable.
- For paths containing backlink expansion, reject FIRST/LAST in the first implementation because there is no source-array order. MIN/MAX remain supported. A future explicitly defined backlink order is separate work.

Missing fields in valid targets follow the existing path-selection contract. Dangling references and targets lacking logical data remain unavailable, not ordinary empty arrays.

Relations may cycle in the declaration graph, but only explicit finite paths are evaluated. Retain existing depth/hop and resource limits. Sorting does not recursively expand all reachable records.

If the old schema adapter supports x-sort-reduce, translate it to the same normalized reduction where applicable. Explicit request reduction takes the documented request role; reject contradictory forms rather than depending on parse order.

### 6.5 Collation and transformations

Prepare collators once per effective configuration and reuse them.

- Preserve current locale, strength, numeric-ordering and null/missing options.
- Do not compare key bytes made with incompatible collation configurations.
- Keep early rejection of incompatible cross-collection configurations unless the caller supplies a supported explicit common override.
- Include ICU/version/configuration in compatibility.
- A registered text operation must expose a stable scalar sortable representation. An opaque Boolean matcher is insufficient.
- Preserve raw versus transformed semantics: body and body:normalized may sort differently.
- Decline unsupported operation/sort pairings with precise errors.
- This work consumes existing renderers/bindings; it does not implement Markdown/XML/HTML rendering.

### 6.6 Computed sorting execution

For unindexed sorting, a small LIMIT does not generally avoid examining every matching key. Make that visible in EXPLAIN and measurements.

Execution requirements:

1. Apply safe FILTER/FTS candidate restriction before computing expensive keys.
2. Fuse residual verification and extraction of all needed local sort keys in one decode where possible.
3. If membership is proven by a posting, still fetch the required computed key; do not treat membership proof as key availability.
4. Keep compact identity/slot/key winners for offset + limit.
5. Retain bodies only within the explicit payload budget; never all matching documents.
6. Group source reads by file and reuse projections.
7. Deduplicate related target extraction within the declared query-local budget. Shared targets must not trigger a complete target query per source record.
8. Reuse already available terminal values/keys across predicates, sort terms and event publication where semantics agree.
9. Track bytes of variable-size keys as well as winner counts.
10. Bound worker-local heaps, coordinator heap, target caches and pending results together. Do not describe workers × page capacity as one page of memory.
11. On exceeded limits, return an explicit resource error, never a silently truncated successful page.

Deep offsets necessarily increase top-k retention. Use existing checked arithmetic and report that cost. External sorting/spill files are not required; a documented byte limit with a clear error is acceptable.

When a matching record contains a runtime-invalid requested sort value, preserve that error even if it would otherwise fall outside the page. A zero-length requested page retains no winners/payloads; query binding and any required runtime expression validation follow the same documented collected/streamed contract.

### 6.7 Composite sort indexes

Add explicitly configured composite ordered indexes as a separate milestone. They must not be created by ordinary inference or an arbitrary query.

The required first version supports local declared scalar fields and metadata with compatible existing sort-key definitions. Related or reduced composite indexes may be rejected explicitly; computed relation/reduction sorting remains required.

A composite definition includes ordered expressions, directions, collation/binding semantics and exceptional-value placement. Build it from the existing prepared keys after required columns are available, without reparsing every source.

Compatibility must include:

- Normalized expressions and direction vector.
- Reducers and binding versions, where supported.
- Effective collation and ICU version.
- Null/missing policy and identity tie-break.
- Record-slot and configuration generations.

Exact declared-order reuse is required. Prefix or reversed-order reuse is optional only with a proof and tests; do not assume reversing an ascending vector preserves null placement or ties. Keep existing single-column order paths.

Restore validates a permutation/domain/order invariant and configuration identity. Do not trust a composite slot vector against a different generation.

### 6.8 Sorting caches

Distinguish:

- Complete verified membership.
- A bounded ordered page.
- A complete ordered result.

A computed top-k page is not complete membership or a full ordered list. Do not retain all dynamic keys just to build a cache entry.

Membership cache hits can still require computed sort-key reads. Expose that work. Where an existing complete ordered entry fits the budget, reuse it across offsets and limits.

Order identity includes normalized paths, reducers, bindings, directions, collation, exceptional-value policy and all relevant source/target generations. Existing conservative repository-generation scoping is acceptable. Partial streaming pages cannot establish complete ordered-cache entries.

### 6.9 Sorting acceptance

Use an independent small full-sort oracle and fixed expected IDs, not only the production comparator.

Required cases:

- Undeclared local/nested scalar values, no inference and no sort registration.
- Metadata sorting without columns.
- Every supported scalar kind and mixed kinds across records.
- Large numbers, fractions, Unicode, equal collation keys and stable identity ties.
- Null/missing/unavailable policies in both directions.
- Positional arrays and every reducer, including empty/null/duplicate/nested cases.
- Forward and backlink paths, cycles through finite hops, dangling and fallback targets.
- FIRST/LAST occurrence order deliberately different from target ID order.
- Multiple keys, mixed directions and inline/programmatic parity.
- Sort-only and FTS/FILTER combinations.
- Inference disabled/report-only/auto-index, warm/disabled cache, fresh/restore.
- Incompatible cross-collection collation and unsupported object/multivalue errors.
- High fan-out and many sources sharing a few targets.
- Composite compatibility and rejection cases.
- Small pages, zero limit and deep offsets.

Counters must show retained early stopping for eligible stored ordering, full relevant key evaluation for computed ordering, bounded retained bytes and target reuse. Never claim that a dynamic LIMIT 20 query reads only 20 matching records.

## 7. Streaming query results

### 7.1 Public execution contract

Add a synchronous streaming/sink API around the same execution machinery used by collected search. No new async runtime is required.

The conceptual API is:

~~~rust
search_stream(request, stream_options, cancellation, sink)
    -> Result<StreamOutcome, StreamError>
~~~

This is an interface requirement, not a claim that the function already exists. The supervisor pins concrete Rust types at the shared checkpoint.

- Invoke the sink serially on the caller/coordinator thread.
- Never invoke it while holding repository/cache locks.
- Borrow event slices only for the callback duration.
- Return explicit Continue or Cancel control from callbacks.
- Allow an externally shareable cancellation token for periods with no callbacks.
- Distinguish sink failure, execution failure and cancellation.
- Keep collected search as a consumer of the same binding, membership and ranking logic.
- Do not implement streaming as search followed by callbacks over its finished result.
- Do not copy the whole query executor into a second streaming-only implementation.

The ordinary collected path must not allocate stream event wrappers or queues unnecessarily when streaming is unused.

### 7.2 Delivery modes

| Mode | Contract |
| --- | --- |
| OrderedRows | Append verified requested-page rows only when their positions in the final order are established. |
| PageReplacements | Publish the currently best requested page as a bounded replacement snapshot; ranking remains provisional until completion. |

An event includes an increasing sequence number and the information needed to reconstruct the requested page. PageReplacements replace the previous snapshot in full. A row may move or disappear.

Every emitted row must already have verified query membership, correct requested evidence and a valid materialized payload under the existing read contract. Ranking can be provisional; false-positive membership cannot.

OrderedRows:

- Use inventory identity order or a compatible stored-order cursor with a correct cross-collection merge.
- Worker completion order is never result order.
- Ordered rows remain provisional as a successful whole-query result until terminal completion; a later read/evaluation failure must remain visible.
- For an unindexed computed sort, this mode may wait until all relevant keys are evaluated. EXPLAIN must state that first-row delivery is blocked by ordering.

PageReplacements:

- Supports actual early feedback during computed sorting.
- Emit only at bounded checkpoints and when the visible page changes.
- Do not clone or materialize offset + limit bodies after every matching record.
- Reuse compact winners and bounded payloads; materialize only the visible requested page.
- The successful terminal summary identifies the final page sequence. If the page never had an event, the contract must still explicitly identify an empty final page.

Both modes stream the requested page, not the entire matching set. Separate repeated pagination remains the caller's responsibility. Do not create a hidden full-export mode.

### 7.3 Termination and error handling

Use one terminal source of truth:

- Core returns Complete(summary) on success.
- Core returns Cancelled(partial_summary) on cancellation.
- Core returns a distinguishable execution or sink error on failure.
- Core sink events are nonterminal.
- CLI translates the return into one complete, cancelled or error record if its output sink remains usable.

Do not promise an error event after stdout or the callback itself has failed. Do not emit a successful completion after cancellation/error. No data callbacks occur after terminal outcome.

Already delivered events do not roll back external caller actions. Document that consumers should commit a final result only after Complete. No new verified-membership or ordered-result cache entries are published on a failed or cancelled stream. Prepared-query and matcher caches may retain valid preparations; cancellation does not require rolling those back.

If the sink fails after some verified rows, report delivered/observed progress in the error context without claiming the result is complete.

### 7.4 Counts, ordering and execution scope

Preserve offset, limit, COUNT, return mode, evidence and the common sort comparator.

- Apply offset after ranking, not to worker completion order.
- A zero limit emits no data events.
- An empty result still completes.
- Successful COUNT equals collected COUNT exactly.
- Interim observed_matches is a lower bound unless proven complete.
- Distinguish processed candidates, verified matches, emitted rows and total membership.
- An ordered page may arrive before COUNT finishes.
- Without COUNT, preserve legitimate early termination; do not finish the full scan merely to fill a complete-membership cache.
- A computed sort may need all matching keys even without COUNT.
- Freeze parameters, clock, regex, source/target generations and sort semantics once.
- Do not restart relation queries or rebind temporal macros per event.

The common default order remains collection name and record ID. All explicit new sorting rules apply identically to streamed and collected requests.

### 7.5 Backpressure, cancellation and memory

A slow callback applies backpressure. Do not enqueue an unbounded producer stream.

Bound and account for:

- In-flight scan work.
- Completed but not yet delivered batches.
- Reorder buffers.
- Local and global page winners.
- Variable-size sort keys.
- Related target projections/keys.
- Payload bodies.
- Materialized event bytes.

A row-count cap alone does not bound arbitrary record sizes. Define an oversized-record policy: either one oversized record under a separate hard ceiling or an explicit resource error. Never advertise a bound that excludes event and worker payload copies.

Check cancellation:

- Before expensive binding/planning stages where practical.
- Before source and relation-target batches.
- Between records in long scans.
- Before callback invocation and after it returns.
- During cached-page materialization.

Cancellation does not interrupt an uninterruptible system read or third-party parser midway; document that limit. Stop scheduling new work and join/drain existing workers safely. Avoid busy polling.

Throttle progress and provisional pages using work/batch thresholds, with deterministic test controls. Time-based throttling may supplement those controls but must not make correctness tests flaky.

For related sorts and predicates, maintain one appropriate query-local reuse scope across event boundaries. Reopening target execution for every event defeats the batching contract.

### 7.6 Cache behavior

Existing complete cached membership may be consumed progressively; only requested page payloads are materialized.

A newly computed complete membership/order entry may be published only after:

1. Membership is actually complete.
2. Execution has succeeded.
3. All required sink deliveries have succeeded.
4. Cancellation has not occurred.

Define one completion decision point after the final required sink callback succeeds and before verified-result publication. Cancellation observed before that point produces Cancelled without publication. A cancellation arriving after that decision cannot retroactively invalidate Complete. Use the same decision for terminal status and cache publication, and test the race with deterministic synchronization.

A successful page-only early stop is not complete membership. A provisional top-k page is not a complete ordered result. A cancelled stream must not publish partially collected IDs under a completed query key.

Do not hold the verified-cache mutex while calling user code. A sink must not deadlock merely because it reads repository metadata or makes another independent query.

Delivery mode, event batch size and progress cadence do not change predicate membership identity. Sort and execution-generation dependencies still do.

### 7.7 CLI and application integration

Provide an NDJSON event output option through the current example/CLI request path. Preserve existing collected output and keep diagnostic logs on stderr.

Events must have a documented version, type and sequence. Required transport types are ordered rows/batches, page replacement, optional progress, complete, cancelled and error. Never interleave ordinary human-readable logs into stdout events.

Use existing TUI query execution integration only where practical: expose a thin event adapter and documented cancellation path. A new UI layout or full TUI rewrite is not part of this assignment. The public API and runnable CLI event example are mandatory even if rich TUI presentation is deferred.

### 7.8 Streaming acceptance

Reconstruct the final page from events and compare with:

- Ordinary search on the same execution-bound request.
- An independent expected-answer fixture.
- The independent sorting oracle where ordering is computed.

Cover both alphabets, inference modes, parameters, frozen clocks, regex, local/relation predicates, fallback records, all payload/evidence/count modes, offsets, zero limits, ties, mixed collections, single/composite orders and computed/reduced/related sorts.

Deterministic resource tests must establish:

- Ordered delivery before the remaining candidate domain is fully scanned on an eligible ordered path.
- A provisional computed-sort page emitted early and later corrected when a better record appears.
- COUNT can continue after page delivery without retaining unrelated payloads.
- Cancellation before execution, during a scan, during target work, at a callback and during cache materialization.
- A late source/decoder/operator failure remains visible after earlier delivery.
- Sink errors do not become successful completion.
- No callbacks after termination and no leaked worker/handle state.
- No complete cache publication on cancellation or failure.
- Bounded pending bytes with a deliberately slow sink and multiple workers.
- Collected execution does not pay significant disabled-stream overhead.

## 8. Supervisor, workers, subagents and final merging

### 8.1 Execution authority and completion

The user has requested a supervised implementation with merging at the end. When this specification is supplied for execution, it authorizes:

- Creation of the isolated worktrees and feature/integration branches named below.
- Local commits and pushes to those task branches when the configured remote permits.
- Supervisor merging of reviewed feature branches into the dedicated integration branch during the final integration phase.
- Promotion of the validated aggregate into development/local-file-search-core after the final acceptance gate.
- A normal non-force push of that development result when permitted.

Do not add a routine approval pause before these already requested merges. This supersedes the earlier query-extension assignment's no-automatic-merge rule for this new assignment only. Repository protections and actual tool approval blocks still apply.

Do not merge into master, rewrite published history, force-push, reset unrelated changes, discard another worker's modifications or resolve conflicts by keeping one entire side. If protected-branch policy prevents promotion, preserve the tested integration branch and create a concrete pull request with the evidence; report the exact blocker. Do not describe an unmerged PR as a completed development merge.

The default target is development/local-file-search-core because that is the active implementation branch. Reconfirm it from the repository's current instructions before execution; do not substitute master merely because it is the default GitHub branch.

### 8.2 Worktree layout

Use /home/simon/source as the worktree root. Every Git worktree must be a direct child of that directory. Never create worktrees under /tmp, /var/tmp or another temporary location, including for review, tests or retries.

| Role | Branch | Worktree |
| --- | --- | --- |
| Supervisor | integration/optional-indexing-sorting-streaming | /home/simon/source/objekte-optional-indexing-sorting-streaming |
| Dynamic FILTER worker | feature/dynamic-filter-bigrams | /home/simon/source/objekte-dynamic-filter-bigrams |
| Inference worker | feature/optional-field-inference | /home/simon/source/objekte-optional-field-inference |
| Sorting worker | feature/general-sort-expressions | /home/simon/source/objekte-general-sort-expressions |
| Streaming worker | feature/progressive-query-results | /home/simon/source/objekte-progressive-query-results |

If a path/branch already exists, inspect its ownership, base and status. Resume the correct work rather than deleting or resetting it. Use an explicit descriptive alternate name only when it is unrelated; record the deviation.

Illustrative setup, after checking the original checkout:

~~~bash
cd /home/simon/source/objekte
git status --short
git worktree list
git fetch origin

IMPLEMENTATION_BASE=$(git rev-parse origin/development/local-file-search-core)
git worktree add -b integration/optional-indexing-sorting-streaming \
  /home/simon/source/objekte-optional-indexing-sorting-streaming \
  "$IMPLEMENTATION_BASE"
~~~

The supervisor prepares and commits TASK-00 and TASK-01 in its worktree, validates that checkpoint, and records CONTRACT_COMMIT. Create every feature worktree from that same immutable checkpoint:

~~~bash
CONTRACT_COMMIT=$(git -C /home/simon/source/objekte-optional-indexing-sorting-streaming rev-parse HEAD)

git worktree add -b feature/dynamic-filter-bigrams \
  /home/simon/source/objekte-dynamic-filter-bigrams "$CONTRACT_COMMIT"

git worktree add -b feature/optional-field-inference \
  /home/simon/source/objekte-optional-field-inference "$CONTRACT_COMMIT"

git worktree add -b feature/general-sort-expressions \
  /home/simon/source/objekte-general-sort-expressions "$CONTRACT_COMMIT"

git worktree add -b feature/progressive-query-results \
  /home/simon/source/objekte-progressive-query-results "$CONTRACT_COMMIT"
~~~

These commands are a template, not permission to overwrite existing work. Record the actual full SHAs, paths and remote.

Use distinct build output directories per writing worktree. Do not manipulate another worker's target directory or shared Git references. Ordinary test fixtures may be isolated under the worktree's test/target area; they must not become hidden shared working copies.

### 8.3 Concurrency and supervisor responsibilities

Use one primary writer per worktree. A native subagent shares a filesystem unless explicitly isolated; it is not automatically a Git worktree.

Start with at most three concurrent writing workers:

1. Dynamic FILTER.
2. Inference.
3. Sorting.

Start streaming when a writing slot becomes available. It still starts from CONTRACT_COMMIT and the agreed sort/delivery interfaces. It must not require the sorting worker's unreviewed changes in its filesystem.

The supervisor may research, review and maintain integration documentation while workers run. Avoid simultaneous benchmark runs and competing cargo builds. Increase concurrency only when measured useful throughput and CPU/RAM/disk capacity justify it.

Supervisor owns:

- Base selection and validation provenance.
- Shared semantic contracts and interface-change decisions.
- Task ledger and worker scheduling.
- Independent review assignment.
- Shared-file conflict resolution in the final integration phase.
- Snapshot/cache version reconciliation.
- Final CLI/example integration.
- Cross-feature correctness and resource tests.
- Final performance measurements and report.
- Merges into integration and development.

Workers own their scoped implementation, tests, documentation, commits and handoff. A worker may make minimal compiling edits to shared entry points on its own branch, but must list them explicitly. It must never edit another worktree to “help” integration.

### 8.4 Subagent assignments

Each worker uses bounded subagents where available. Subagents must have a concrete question, owned files if any, and a finite expected result.

| Worker | Required independent subagent work |
| --- | --- |
| Dynamic FILTER | Derive candidate-superset fixtures and prove empty-selection preservation from the selector semantics. A second review checks custom-reader coverage, Boolean composition and observable errors. |
| Inference | Derive expected summaries and membership counts independently. Review admission/budget behavior, parallel merges and snapshot coverage. |
| Sorting | Specify independent mixed-type/reduction/related-key expected order. Review comparator laws, target batching, retained key/payload bytes and stored-order compatibility. |
| Streaming | Design cancellation/late-error/slow-sink fixtures. Review terminal outcomes, cache publication, callback locks, reordered workers and progressive delivery. |
| Supervisor | Cross-feature reviewer checks actual merged code against this specification; performance reviewer interprets the recorded raw samples. |

Read-only subagents may share the parent worktree. A subagent writing a test must own a distinct named test file; serialize changes to common fixtures and modules. Additional production-code writers require separate worktrees and explicit supervisor assignments.

Do not let review subagents implement unreviewed fixes in another worker's scope. Return findings; the owner fixes them and records new validation evidence.

If native subagents are unavailable, preserve the ownership and review model through separate sessions or sequential independent review. Do not block the implementation on a particular orchestration interface.

### 8.5 Task ledger

Maintain docs/development/optional-indexing-sorting-streaming-ledger.md on the integration branch.

States: not started, ready, in progress, ready for review, changes requested, accepted, merged, validated, complete, blocked.

A worker stopping is not completion. An accepted branch has a fixed reviewed head, passing evidence and resolved blocking findings. A merged branch is not yet a validated combined result.

Record every shared-interface change and notify affected workers. An interface change must not silently rebase another worker. If a checkpoint correction is essential, issue a single small named contract commit and explicit application instructions; record it in each branch's ancestry/handoff. Feature merging still occurs in the final phase.

### 8.6 Work packages

#### TASK-00 — Select base, verify prerequisites and retain baseline

Owner: supervisor.

- Read instructions, current source and the prior query-extension reports.
- Recheck development/integration ancestry, working trees and experimental inference branch.
- Confirm relations, parameters, regex and source fallback are present in the chosen base.
- Record existing test/format/Clippy failures separately from introduced failures.
- Pin source/corpus/schema/compiler/options and decide which existing benchmark recordings are comparable.
- Capture a missing baseline only when necessary; do not rerun large benchmarks repeatedly.

Exit: fixed base SHA, baseline disposition, existing issues, active task ledger and ownership map.

#### TASK-01 — Shared contracts and schema-free example foundation

Owner: supervisor.

- Pin the Section 3 interfaces and the Section 6/7 exact sort/stream contracts.
- Provide the minimal general folder-definition helper and schema-free Musenalm example configuration.
- Enable recursion in that helper; preserve low-level discovery compatibility.
- Add path-based identity support needed for duplicate basenames if no equivalent current option exists.
- Expose inference mode only with coherent default Disabled behavior; do not expose successful no-op AutoIndex.
- Prepare narrow construction observation and ranking/delivery integration contracts.
- Commit and validate CONTRACT_COMMIT.

Exit: independently compiling foundation, tests proving no schema-file reads/typed postings in the example, and stable worker APIs. Do not hide foundation overhead by comparing later measurements only against this new checkpoint.

#### TASK-02 — Sound dynamic FILTER candidate planning

Owner: dynamic FILTER worker.

Primary ownership: focused dynamic-candidate helper, executor eligibility/estimation, structural bitmap implementation and tests. Minimal build/storage integration edits are reported.

- Implement Section 4.
- Preserve dynamic binding and complete residual verification.
- Add structural bitmap construction, parallel merge, persistence and coverage.
- Extend EXPLAIN/counters and independent candidate-superset tests.

Exit: exact parity, both alphabets, restore tests, selective parse reduction with inference disabled, broad empty-array cost behavior and worker handoff.

#### TASK-03 — Inference summaries and explicit opt-in policy

Owner: inference worker, first stage.

Primary ownership: inference report/policy/path observation modules and tests.

- Implement Disabled, ReportOnly and AutoIndex configuration contracts.
- Implement observation statistics and canonical path handling.
- Separate declarations, observations and index decisions.
- Establish explicit budget defaults and deterministic drop/admission policy.
- Collect observations during existing parsing/traversal.
- Add source-independent fixtures and custom-reader coverage tests.

Exit: correct complete/incomplete reports, ReportOnly without inferred postings, Disabled without inference-owned catalog allocations, no additional parse pass and bounded merge state.

#### TASK-04 — Optional inferred postings, planning and persistence

Owner: inference worker, second stage on the same branch.

Primary ownership: inference posting admission/building and focused planner adapter; minimal posting/build/storage/repository edits are reported.

- Build admissible Boolean, number and low-cardinality string postings.
- Preserve explicit opt-outs and avoid duplicate compatible indexes.
- Use coverage-qualified inferred indexes without changing query binding.
- Persist policy/report/index compatibility and validate restore.
- Add inference-mode parity, parameter variation and budget-exhaustion tests.

Exit: Disabled/ReportOnly/AutoIndex result equivalence and measurable selective acceleration, no false exact proofs, correct restored behavior.

#### TASK-05 — Computed sort expressions and bounded key extraction

Owner: sorting worker, first stage.

Primary ownership: sort grammar normalization, bound sort/key/reduction modules, sorting comparator and tests.

- Implement the Section 6 expression grammar and programmatic equivalent.
- Fix total ordering before accepting mixed dynamic keys.
- Implement local/metadata/positional/related/reduced keys.
- Reuse compatible transformations and relation resolution.
- Preserve current resident-column fast paths.
- Implement bounded computed top-k and target reuse through the agreed common ranking interface.

Exit: independent full-sort oracle parity, comparator laws, no field registration requirement, reduction/related semantics, bounded keys/payloads and explicit scan cost.

#### TASK-06 — Explicit composite orders and sorted cache compatibility

Owner: sorting worker, second stage.

Primary ownership: composite order builder/reader and sort-order cache integration; shared snapshot edits are reported.

- Add explicit local scalar/metadata composite definitions.
- Build from existing keys without additional source parsing.
- Validate exact compatibility, restore invariants and order reuse.
- Reconcile complete membership, page and full ordered-result cache identities.
- Make EXPLAIN distinguish stored single/composite order and computed top-k.

Exit: correct compatible reuse, incompatible rejection, restored order parity, retained single-column early stop and no accidental all-match key cache.

#### TASK-07 — Progressive execution, cancellation and transport

Owner: streaming worker.

Primary ownership: streaming.rs, sink/outcome/cancellation types, delivery tests and CLI event adapter. Changes to run.rs use the shared consumer/ranking contract.

- Implement Section 7 against the stable baseline and supervisor interfaces.
- Deliver actual early ordered batches and provisional page replacement.
- Preserve grouped source reads, target reuse, evidence and payload contracts.
- Add bounded queues/reordering, backpressure and cancellation.
- Make new cache publication depend on successful complete execution and delivery.
- Keep ordinary search on the common execution machinery.

Exit: independent final-page reconstruction, true first-event evidence, late-error/cancellation/cache tests, slow-sink byte bounds and current collected-search parity.

Before final integration, demonstrate provisional ranking with baseline stored keys or a bounded test key provider through the agreed interface. Do not duplicate the sorting worker's computed/reduced/related implementation just to satisfy an independent branch test. Full new-sort streaming parity is a mandatory TASK-09 integration gate.

#### TASK-08 — Independent worker review and release candidates

Owner: supervisor plus read-only review subagents.

- Review exact branch heads and diffs, not only handoff prose.
- Check each exit condition and test evidence.
- Resolve review findings on the owning worker branch.
- Confirm every branch compiles independently against the agreed checkpoint.
- Record all shared-file changes and version-number conflicts before merging.
- Freeze accepted worker heads as integration inputs.

Exit: all four implementation branches accepted, no unresolved blocking correctness/resource findings. No feature merge is required before this gate.

#### TASK-09 — Final merge, cross-feature repair and validation

Owner: supervisor, integration worktree only.

This task is mandatory and merges the work; it is not a plan for a future unidentified agent.

Merge accepted heads in this default order:

1. Dynamic FILTER.
2. Optional inference.
3. General sorting and composite orders.
4. Progressive streaming.

Use explicit accepted commit SHAs, preserve branch history with ordinary merges, and record the resulting merge SHAs. Resolve conflicts semantically. After each merge, run focused integration checks and a compile check before continuing.

Required reconciliation:

- Structural empty-array facts and inference share the intended extraction pass.
- Inference Disabled remains the default everywhere.
- Unknown field binding is not replaced with inferred declarations.
- Sort key extraction, ranking and streaming use one comparator.
- New computed sorts work on cached membership without resident-column assumptions.
- Streaming pages use the same reducers and relation target batching.
- Snapshot/configuration versions include every new persisted semantic change.
- Resource budgets count the combined live state rather than each subsystem in isolation.
- Public request changes, CLI options and diagnostics compose coherently.

Run the full cross-feature matrix in Section 9. Fix discovered errors in explicit integration commits. Do not weaken expectations to make a merge pass.

Exit: validated final code candidate, complete conflict-resolution record, no remaining unintegrated required worker commit.

#### TASK-10 — Final measurements and report

Owner: supervisor; measurement subagent may analyze recordings.

- Run Section 10 once on the final validated code.
- Save exact per-query samples, expected-result checks, source/target counters and memory/construction results.
- Identify the exact measured code parent and configuration.
- State limitations, advisory warnings and any material regressions.
- If code changes after timing, rerun affected checks and measurements.

Exit: reproducible report and complete evidence tied to the final implementation, not stale worker benchmark claims.

#### TASK-11 — Promote to development and deliver

Owner: supervisor.

- Fetch development again and inspect changes since IMPLEMENTATION_BASE.
- If development has advanced with unrelated commits, merge those changes into the integration branch, resolve deliberately, and rerun affected correctness/performance checks.
- Never reset development to the old base.
- Ensure the final integration candidate includes current development and all accepted work.
- Promote development with a fast-forward when possible. If repository policy requires a normal merge or PR, follow that policy without force updates.
- Push normally if permitted; record exact local/remote heads.
- Verify ancestry of all accepted worker commits in the promoted result.
- Leave worktrees and user files intact; do not delete branches as an unsolicited cleanup step.

Exit: merged development commit and final handoff, or a concrete tested PR/blocker if branch protection prevented promotion. A report saying “all workers finished” is not this exit.

### 8.7 Commit milestones

Use independently reviewable commits; messages below are suggested professional names, not a demand for exactly this chronological history.

| Milestone | Owner | Suggested commit message |
| --- | --- | --- |
| S00 | Supervisor | docs(core): record search extension contracts and baseline |
| S01 | Supervisor | feat(source): add recursive schema-free folder definitions |
| S02 | Supervisor | feat(identity): support relative source path identities |
| A01 | Dynamic | feat(index): retain empty-array structural coverage |
| A02 | Dynamic | perf(query): restrict dynamic string predicates with shared bigrams |
| A03 | Dynamic | test(query): verify dynamic candidate supersets and restore |
| B01 | Inference | feat(inference): collect bounded optional field observations |
| B02 | Inference | feat(index): build opt-in inferred typed postings |
| B03 | Inference | feat(storage): persist inference policy and complete coverage |
| B04 | Inference | test(inference): verify semantic parity across indexing modes |
| C01 | Sorting | feat(query): bind computed sort expressions and reductions |
| C02 | Sorting | fix(sorting): define total ordering for dynamic scalar keys |
| C03 | Sorting | feat(sorting): evaluate local and related sort keys in bounded batches |
| C04 | Sorting | feat(index): add explicit composite sort orders |
| C05 | Sorting | fix(cache): separate computed pages from complete ordered results |
| D01 | Streaming | feat(query): add streaming sink and cancellation contracts |
| D02 | Streaming | feat(query): deliver ordered batches and provisional sorted pages |
| D03 | Streaming | fix(cache): publish stream results only after successful completion |
| D04 | Streaming | feat(cli): expose versioned query event output |
| V01 | Supervisor | test(core): verify inference sorting and streaming interactions |
| V02 | Supervisor | docs(bench): record search extension correctness and performance |

Every behavior-changing commit includes relevant focused tests and repository-required checks. Snapshot/cache compatibility changes occur in the commit that first changes the format or semantics, not in a later cosmetic version bump. The supervisor reconciles independent branch version changes during final integration.

### 8.8 Worker handoff format

~~~text
Task identifiers:
Branch and worktree:
Pinned base and contract commits:
Accepted implementation head:
Ordered commit SHAs:
Owned and shared files changed:
Implemented behavior:
Explicitly excluded behavior:
Public API changes:
Snapshot/cache compatibility changes:
Test commands and exact outcomes:
Independent oracle or candidate-superset evidence:
Resource-counter evidence:
Performance samples, if measured:
Known warnings, failures and limitations:
Subagent findings and their disposition:
Shared-interface deviations:
Integration notes:
~~~

Workers commit and push completed steps when the configured remote is available. Do not accumulate the whole implementation into one final commit. A push failure is reported accurately and does not justify force-pushing.

## 9. Combined correctness and resource acceptance

### 9.1 Semantic matrix

Use small fixtures to cover the cross-product selectively, without generating an unnecessarily huge duplicate test suite.

| Dimension | Required cases |
| --- | --- |
| Index policy | Disabled, ReportOnly, AutoIndex; declared and undeclared fields |
| Text index | ASCII and Unicode; useful/absent grams; empty-array preservation |
| Source | JSON, JSONL, non-JSON test reader; logical/fallback/unreadable states |
| Query | FILTER, FTS, regex, mixed FILTER/FTS; parameters and frozen time |
| Paths | Nested, positional, arrays, forward links, backlinks |
| Order | Default, stored scalar, stored composite, dynamic, reduced, related |
| Delivery | Collected, ordered rows, provisional replacement |
| Payload | Locator, original bytes, logical value, evidence on/off |
| Page/count | Zero/one/many hits, offsets, deep pages, COUNT on/off |
| Lifecycle | Fresh, restored, warm/disabled verified cache |
| Failure | Read/decode/operation error, sink error, cancellation, resource exhaustion |

Require explicit expected answers where production and reference execution share helpers. A shared comparator or selector is not an independent oracle.

Use property tests with recorded seeds for:

- Candidate supersets.
- Comparator laws.
- Top-k versus independent complete sorting.
- Inference admission without result changes.
- Final stream reconstruction versus collected execution.
- Snapshot round trips with preserved slot domains.

Minimize and retain failing cases.

### 9.2 Critical interaction fixtures

At minimum:

1. An undeclared universal string predicate with an unrelated empty array; structural overinclusion must be rejected by the residual.
2. An undeclared string predicate whose collection also contains raw fallback records selected by a metadata OR branch.
3. Inference admits a numeric posting while an unrelated path changes kind across records; accepted logical data and query answers remain unchanged.
4. Same query and parameter values with inference disabled/enabled, both producing the same dynamically sorted stream.
5. Many source records share a few related targets used by FILTER and sorting; event publication must not restart target decoding.
6. Computed related MIN sorting with null, missing, dangling and unavailable targets.
7. A late better sort key replaces an early provisional page.
8. A source error or cancellation after the first provisional page produces no success or new complete cache entry.
9. Warm complete membership plus an unregistered sort still performs required key extraction and reports it.
10. A stored composite page streams early and retains stable cross-collection ties.
11. Multiple workers, slow sink, large record bodies and deep offset stay within the declared combined budget or return a resource error.
12. Reopening with inference Disabled does not activate persisted inferred indexes from a prior AutoIndex opening.

### 9.3 Required checks

Follow current repository instructions, including workspace tests and Clippy before behavior commits.

At accepted worker heads and final integration:

~~~bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets
git diff --check
~~~

Apply Rust formatting and then the repository spacing formatter:

~~~bash
cargo fmt --all
python3 scripts/format-rust-spacing.py
python3 scripts/format-rust-spacing.py --check
~~~

Inspect the diff before committing. The inspected repository has recorded pre-existing formatting drift and advisory Clippy warnings. Do not silently commit unrelated whole-workspace reformatting. Record baseline drift, keep touched code compliant, and report advisory warnings accurately. Do not invent a warning-free result.

Run the existing relevant acceptance/oracle commands recorded by the repository. Use deterministic counters instead of millisecond assertions for resource and early-delivery tests.

## 10. Measurement protocol

### 10.1 Baseline discipline

Record code SHA, corpus and schema fingerprints, collection definitions, compiler/build profile, CPU/worker settings, bigram alphabet, cache budgets, inference/sort policies and timing boundaries.

Reuse a prior recording only when those factors are compatible. The earlier query-extension final report is evidence for its measured parent, not automatically for the newer development head.

During worker development, run focused fixtures and microbenchmarks. Do not rerun the full 150-query workload after every commit.

After integration, run the existing 150 Musenalm workload unchanged, using both alphabets, verified cache disabled and the established 64 MiB warm profile, and the repository's established repetition count. Preserve all per-query rows and raw samples. Maintain the existing recent feature workload where applicable; add one separate compact workload for these new features.

Do not benchmark while other agents compile, index or run heavy tests. Record unavoidable contention instead of presenting noisy results as improvements.

### 10.2 Construction and inference measurements

Measure the same schema-free folder collection with:

1. Inference Disabled.
2. ReportOnly.
3. AutoIndex.

Record:

- Time to searchable readiness.
- File reads and record parses.
- Structural summary time/bytes.
- Inference observation, provisional index and merge time.
- Number of observed/admitted/dropped paths and reasons.
- Retained and peak process memory.
- Final inferred posting bytes and structural bitmap bytes.
- Snapshot size and serialization time.
- Fresh-process restore-to-ready time.
- Time and extra work when enabling inference through a later reopen.

Keep the schema-aware established workload separate from the schema-free experiment. Do not silently remove declarations from the old benchmark and call it the same baseline.

### 10.3 Query and sorting measurements

The compact workload includes:

- Selective and broad undeclared equality/contains.
- Many empty arrays and absent fields.
- Numeric/Boolean/enum-like filters with inference disabled/enabled.
- High-cardinality strings and dropped inference candidates.
- Broad stored scalar and composite order pages.
- Selective and broad unregistered sorts.
- Mixed-type keys, array reductions and related keys.
- Shared related targets and high fan-out.
- Small pages and deep offsets.
- Warm membership cache with computed sorting.

Separate binding/planning, candidate work, source/target verification, key extraction, ranking and payload materialization where counters permit.

Report total match count, source/target records read, parses, dictionary/edge work, key bytes, retained winners and cache outcomes. Include the cost of source reads required by unindexed sorting; do not compare it with a precomputed order as though the work were identical.

### 10.4 Streaming measurements

For the same bound requests, record:

- Collected total time.
- Streaming total time with a no-op sink.
- Time to first data event.
- Time to first full requested page.
- Time to terminal completion.
- Callback/backpressure time.
- Number and bytes of events/page replacements.
- Source/target reads and parses.
- Peak pending/reorder/payload/key bytes.
- Cancellation work after the request is observed.

Measure ordered streaming and provisional computed-sort streaming separately. A lower first-event latency does not imply lower total execution time.

### 10.5 Evaluation of regressions

Report absolute timings first, percentages second. Treat roughly 50 ms construction differences as potentially noise unless repeated controlled samples establish otherwise; the user's earlier allocation experiments demonstrated why tiny differences need caution.

Do not promise a sub-second complete build or invent a speedup target unsupported by hardware/corpus data.

Acceptance requires:

- No unexplained material regression in established queries when the features are unused.
- No extra inference traversal/catalog allocation in Disabled mode.
- No whole additional source parse pass for build-time inference.
- Actual source-work reduction for at least one selective undeclared text filter.
- Actual typed-index acceleration for suitable opt-in fields.
- Retained early stopping for eligible resident order.
- Bounded memory and documented full-key evaluation for computed sorting.
- Demonstrably early useful stream events on supported plans.
- Reasonable collected-search overhead when streaming is unused.

If a material regression occurs, investigate plans, allocation and reads. Fix it or describe a concrete measured tradeoff and unresolved acceptance item. Do not mark every feature complete solely because warm verified-cache totals are small.

## 11. Final deliverables and definition of done

Deliver:

1. All required implementation and test commits on accepted worker branches.
2. A dedicated integration history containing those commits and explicit conflict repairs.
3. A merged, validated development result, or the concrete protected-branch PR/blocker described in TASK-11.
4. Updated public API, query grammar, inference policy, sort semantics and streaming documentation.
5. Runnable schema-free Musenalm/folder example and event-output example.
6. Task ledger with subagent findings and disposition.
7. Independent correctness and resource evidence.
8. Reproducible benchmark data and a report tied to the final code parent.
9. A concise handoff identifying the resulting branch/commit, commands run, important behavior changes, performance results and remaining limitations.

The supervisor must not declare completion while required feature branches remain unmerged, while a candidate-superset/comparator/cache correctness issue is open, or while final measurements refer to a different implementation.

The user should be able to open folder-defined collections with no field schema, use safe shared-bigram FILTER acceleration, opt into inferred typed indexes, sort supported unregistered/related values, and consume bounded progressive query results from the combined implementation.

## 12. Source references

Implementation observations were checked against development commit e21be07414ca4765ffa198032090c30de8683402:

- [Collection configuration and repository entry points](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/repository.rs)
- [Dynamic binding, selectors and exact evaluation](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/reference.rs)
- [Existing schema-free query regression](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/reference.test.rs#L608)
- [Planner eligibility and proof handling](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/executor.rs)
- [Construction and default text extraction](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/build.rs)
- [Source readers and logical text coverage](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/source.rs)
- [Discovery policies](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/discovery.rs)
- [Typed posting construction](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/posting.rs)
- [Current sorting](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/sorting.rs)
- [Current execution and cache paths](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/crates/core/src/run.rs)
- [Query-extension ledger and validation provenance](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/docs/development/query-extensions-ledger.md)
- [Repository code guidelines](https://github.com/Simon-Martens/objekte/blob/e21be07414ca4765ffa198032090c30de8683402/docs/development/code-guidelines.md)

Archived behavior is reference material, not code to restore wholesale:

- [Original inference policy and workflow](https://github.com/Simon-Martens/objekte/blob/5e3ceab56846c8bc57473a3fd9a4377f817521b6/docs/inference.md)
- [Original sorting tests](https://github.com/Simon-Martens/objekte/blob/5e3ceab56846c8bc57473a3fd9a4377f817521b6/crates/core/tests/sorting.rs)
- [Original related sorting tests](https://github.com/Simon-Martens/objekte/blob/5e3ceab56846c8bc57473a3fd9a4377f817521b6/crates/core/tests/relation_filters.rs)
- [Original streaming entry points](https://github.com/Simon-Martens/objekte/blob/5e3ceab56846c8bc57473a3fd9a4377f817521b6/crates/core/src/repository/json_query.rs)
