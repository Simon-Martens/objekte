# Objekte path-posting repair specification

**Deliverable.** Repair query membership, segmented facet lookup, inference diagnostics, and inference construction memory on `repair/path-posting-query-evaluation`. The acceptance baseline is `aed69b4bc1b3cb5d8bb97293d4cc1cdb394e96c7`. Produce one integration tip and a short results report for the owner to review. Use the existing repair worktree. If the tip has advanced, inspect the intervening commits and reproduce each finding on the current tip before changing it. Record both revisions.

| Work item | Defect | Acceptance condition |
| --- | --- | --- |
| 1. Empty-string equality | An exact posting offer can discard a matching record. | Optimized and reference record IDs agree for default and existential comparisons. A proven simple field still runs from its posting. |
| 2. Array length | A single field name can select several arrays, or no terminals, in one record. | Length proofs follow the bound query's quantification and the observed selection shape, including array roots. |
| 3. Facet binding | An active segment lacking the path can cause an admitted facet to fail for the entire collection. | Reversing the segment order produces the same keys, counts and coverage. Each segment contributes only its own slots. |
| 4. Occurrence diagnostics | An interrupted observation can publish zero where the surviving record contains two values. | All three occurrence totals expose whether they are exact, upper bounds, or incomplete observations. |
| 5. Construction memory | Sealing copies structural bitmaps into a second field state. The published segment can retain this uncharged data after global admission. | Compute compact admission observations, release them at final publication, and account for them while live. Measure ordinary construction separately from profiling. |

The production query payload remains one `PostingIndex` per indexed path per segment, with `values` (including `ScalarKey::Null`), `lengths`, `presence`, and the existing small summary and capability metadata. **Segment merging is outside this assignment.** Preserve the current segment boundaries and publication policy. Use supported construction, query and facet APIs. The owner handles integration into master.

## 1. Empty-string equality

**Start with a failing public regression.** Use the standard JSON adapter, an empty schema, AutoIndex and one segment. Each row is a separate file and record. Compare stable record identities using an ID-only request with enough room for every result. An equal result count can conceal a swapped record.

| ID | JSON |
| --- | --- |
| A | `{"items":{"name":""}}` |
| B | `{"items":[]}` |
| C | `{"items":{"name":"Ada"}}` |
| D | `{}` |

| Query | Expected IDs |
| --- | --- |
| `items.name = ""` | A, B |
| `items.name ?= ""` | A |

The binder marks undeclared fields as multivalued. B has a successful traversal with zero values. Every selected value passes the default comparison, and no selected value passes the existential comparison. D has a missing path and fails both. The inferred posting can have `multi == false` because its observed names came through objects. That describes its contents. The bound term still determines how B is evaluated.

1. Follow `query_access/catalog.rs` from proposal to restricted execution for `PostingOperation::EmptyStringExact`. Compare it with `index/posting/lookup.rs` in `estimate_exact_with_visits`, `exact_set` and `exact_set_restricted`. The general guard currently declines B's shape, then the special offer returns the empty-key posting as exact.
2. Put the empty-key eligibility check in one place on the posting side. Reuse the existing path, binding, identity-comparison, literal, coverage, scalar-shape and dictionary checks. Require proof that the posting has the same membership as the **bound** comparison. For a multivalued default term, an empty traversal, a terminal array flattened to zero elements, or multiple terminal values can invalidate equality with an existential key bitmap. Accept only if the existing summary and completeness facts rule out the relevant ambiguities. A missing summary supplies no such proof. Keep independently certified providers on their existing contracts.
3. Make the special offer use that proof, or remove the special operation if the ordinary path also covers complete empty-string entries in a partial dictionary. Preserve that useful partial-dictionary optimization when an empty key is independently certified. Check later `ObservedScalarExact`, `StringDictionaryExact` and `ShortStringCandidates` branches for the same term. The short-string residual path is usable only when its candidate set includes every possible reference match, including B. Decline the offer if coverage cannot prove that inclusion. Once an early restriction loses B, later verification cannot recover it. Only exact evidence may be complemented.
4. Test optimized and reference execution with the table above and with an indexed AND restriction whose surviving domain includes A and B. Include a simple scalar fixture that genuinely admits a complete empty-string lookup. For this fixture, check that the selected plan is exact and predicate verification causes zero source opens/parses. Use the ordinary query entry point and its real binder. A unit test that manufactures an index with convenient flags is insufficient.

[Offer and execution](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/query_access/catalog.rs#L144) · [Ordinary estimate](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/index/posting/lookup.rs#L768) · [Reference comparison](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/reference/filter.rs#L62)

## 2. Exact length comparisons

The length dictionary maps an array length to the records containing **at least one selected array** of that length. With a valid complete length index, a union of qualifying keys implements an existential length test. It cannot by itself prove that every selected value passes or that the first selected value passes. Non-array terminals fail the source length comparison.

**Regression fixture.** Use the same reader and empty schema. Write the top-level arrays verbatim. Verify the fixture frames each file as **one record**. Start with A and B so the original false positive is unmistakable. Then run the extended fixture.

| ID | JSON | Selected values for `x` |
| --- | --- | --- |
| A | `[{"x":[]},{"x":[7]}]` | Two arrays, of lengths 0 and 1 |
| B | `{"x":[7]}` | One array of length 1 |
| C | `{"x":[]}` | One array of length 0 |
| D | `{}` | Missing |
| E | `[]` | Successful traversal, zero selected values |
| F | `[[]]` | Successful traversal, zero selected values |

| Query | A and B only | Extended fixture |
| --- | --- | --- |
| `x:length = 1` | B | B, E, F |
| `x:length ?= 1` | A, B | A, B |

1. Replace the path-length shortcut in `length_posting_is_exact`. Give the proof access to the posting and bound term. Retain the current coverage, operator, numeric literal and explicit array-step gates. For a genuine existential evaluation over **all** terminals, complete lengths prove the result. For a first-value evaluation require proof of at most one selected terminal. For a default all-values evaluation require both at most one selected terminal and no possible successful zero-terminal traversal. Bind the rule to the actual reference branch (`term.multi` as well as `term.quantifier`). This conservative rule also handles mixed terminals without a new bitmap. Use it for estimates and both execution methods. Continue to use unions of qualifying length entries, including `!=`; a record can occur at two different keys.
2. Repair the input to this proof. In `construction/inference.rs`, `visit` currently records empty ancestor arrays only when the observed path is nonempty. The finish/observation loops and the global absent-field loop also start at prefix depth one. Consequently E and F can escape a guard that checks only `may_have_empty_traversal()`. Represent the empty root prefix in the **existing construction-only** `empty_ancestors` table when the walker encounters an empty array at the unnamed root path, including recursive traversal through root arrays. Allocate this entry lazily. Apply proper ancestor prefixes including depth zero to nonempty field paths. An empty field path has no proper ancestor. Preserve the existing distinction between a selected terminal array (C) and traversal through an ancestor array (E/F). Use the existing bitmap union to deduplicate records that occur under several prefixes. Apply the same logic in local candidate finishing, collection aggregation for paths absent from a segment, and when computing compact observations in item 5. Preserve current budget and batch slot-offset accounting. Projections may retain a conservative summary; they may never clear a possibility without proof.
3. When a segment has no local posting for the field, retain the normal residual route where a successful empty traversal can match. A globally admitted field does not establish a segment-local empty result. Also test E in a separate segment from B. On incomplete inference, the source evaluator remains authoritative.
4. Compare exact record IDs for default and existential equality, a range, and inequality. Exercise estimate, unrestricted and restricted bitmap paths for the changed helper. Add an object-root control where a single array is selected, including `{"x":[]}` for length zero. Check that a certified simple length comparison and an eligible existential comparison still use the index without source verification. Keep existing positional, trailing-wildcard, `:exists`, `:null` and type predicate regressions green.

[Current length proof](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/index/posting/lookup.rs#L1139) · [Traversal](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/query/semantics/record.rs#L47) · [Inference walker](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/construction/inference.rs#L1564)

## 3. Resolve a segmented facet at collection scope

**Failing public call.** Build two active one-record initial segments under AutoIndex. Segment I has `{"other":true}`. Segment II has `{"x":7}`. Require two distinct active segment IDs in the fixture, an empty base domain and a completed initial build. Request `PostingValuesRequest::new("data", "x", 10)` in both segment orders. Both pages must contain exactly the numeric key 7 with count 1, no continuation, 2 eligible, 1 present and 0 explicit nulls. Compare the **whole coverage object** with an equivalent single-segment control, including indexed/unresolved totals and `value_counts_complete`. The internal visited-key counter is diagnostic.

1. In `repository/posting_values.rs`, separate the decision about **which dictionary the request addresses** from the borrowed posting for a domain. Preserve schema default-binding precedence and registered named/custom versions. For an ordinary base with active records, resolve and validate that base's own posting as today. For segmented initial construction, `collection.inference` can be an empty skeleton. The completed collection admission is in `collection.global_inference`.
2. For an inferred default facet with a finalized collection decision, check the report mode, observation completeness, path admission and facet capability. A path admitted solely for length or structural queries supplies no value dictionary. Preserve complete-value and partial short-string distinctions and policy version. A finalized explicit rejection/opt-out remains effective. For paths introduced by subsequent updates, or while initial construction is in progress, preserve the existing local-admission fallback when there is no applicable finalized decision for that path. ReportOnly and Disabled cannot authorize an inferred facet.
3. If a physical representative is needed for an empty base, search active segments for a **compatible value dictionary**. A segment missing the path is ordinary and cannot terminate the search. Use the collection decision to identify the value family and coverage policy. In the reviewed code, even the aggregate diagnostic's `kind` and `multi` are populated from `indexes.first()`; these alone cannot settle disagreement. For a full homogeneous dictionary use the agreed value family. For a short-string dictionary use the admitted string policy and byte limit. Preserve existing scalar/array representation rules. Where no single compatible enumerable dictionary exists, keep the existing unsupported-coverage outcome consistently in either order. Avoid scanning files to choose the binding.
4. Accumulate coverage and values once for each real active domain using its own posting, active mask and logical slots. An empty base contributes zero. When an active segment needs source fallback, keep `scan_inferred_segment` and its restricted coverage semantics. Preserve ordinary null-key exclusion, legacy presence and singleton-null counting, selection masks, key ordering, pagination and cursor validation. Reuse compatible postings. A missing dictionary in one segment gives no reason to rescan another segment's complete dictionary.
5. Extend existing facet tests for the reversed-order fixture. Run existing partial-short-string, updated/deleted segment, selection and cursor cases. Add one targeted negative assertion that a finalized opt-out or length-only field still cannot become an enumerable inferred facet via a provisional physical posting.

[Facet resolution](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/repository/posting_values.rs#L160) · [Segmented call](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/repository/posting_values.rs#L782) · [Global decision publication](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/construction/repository_state.rs#L545)

## 4. Give the three occurrence totals an explicit contract

The affected public fields on `InferredFieldReport` are `value_occurrences`, `nonblank_occurrences` and `short_string_observed_occurrences`. One record may supply arbitrarily many occurrences. A limit on surviving records cannot bound any of these totals.

Keep the three numeric fields. Add one serializable public enum `OccurrenceCountStatus` and one member `occurrence_count_status` applying jointly to them. Use the following meanings and document them beside the fields. Give the new member a conservative Serde default of `IncompleteObservation` for older reports that lack it. Follow existing enum serialization conventions. Audit constructors, re-exports and public report serialization. If an existing output displays these totals as exact, expose their status there. A public Rust struct gains a field; mention the consequence for external struct literals in the report.

| Status | Contract and calculation |
| --- | --- |
| `Exact` | All three values are exact for the reported domain. Use for a fresh, exhaustive observation with complete accounting of these counters, a proven zero-occurrence domain, or a projection that independently reconstructs all three exactly. |
| `UpperBound` | Each numeric field is at least its true survivor total. Use when projection inherits exact counts from a fully counted parent, or valid upper bounds, without establishing exactness for the survivor set. |
| `IncompleteObservation` | The numbers retain observed/inherited diagnostic tallies. They give no guaranteed upper or lower bound for the current survivor domain. A partial count on the parent may include removed records and miss surviving ones. |

1. Set the status where local and global reports are created. Distinguish a stopped observer or a counter collection mode that omitted contributions from a fully counted domain. `index_status == Built` is not evidence of count accuracy. Verify the actual increments in `FieldState::add_scalar`, including short-string threshold/drop behavior, before assigning `Exact`. Use the conservative joint status if even one of the three lacks the stronger guarantee.
2. Update both indexed and unindexed paths in `finish_projected_inference`, which also serves partition projection. Preserve the incoming status before mutating the report. If all three totals have an independent exact proof, assign `Exact`; if they are inherited, propagate `Exact → UpperBound`, `UpperBound → UpperBound`, and `IncompleteObservation → IncompleteObservation`. A demonstrably identical domain may retain its prior status. A partially observed zero cannot certify absence. Zero eligible survivors does certify three zeros. Preserve the existing proven non-string scalar reconstruction where the posting covers the whole survivor domain and the scalar-kind proof is sufficient.
3. In existing code that combines report fields, use checked count addition and the weakest contributing guarantee (`IncompleteObservation` then `UpperBound` then `Exact`). A proven empty contributor is neutral. This is metadata propagation inside existing callers. Keep the repaired exact-null and record-count bounds intact. Avoid rescanning files, materializing occurrence lists, using a numeric sentinel or introducing new structural postings to improve a diagnostic.
4. Exercise a genuinely interrupted ReportOnly observer with `max_record_contributions = 2`. Observe, in order, `{"x":null}`, `{"stop":{"nested":0}}`, `{"x":["a","b"]}`. Assert that observation stopped before the final input, then retain only the final record. A fresh **unrestricted** observer over the survivor must count `(2,2,2)`. The projected report may keep the inherited `(0,0,0)` only with `IncompleteObservation`. Project again and check it cannot gain authority by repetition. Include an empty-survivor control, a fully observed case with a strict upper bound after removal, and a JSON deserialize case missing the status field. Update the projection oracle to test equality for `Exact`, component-wise ≥ for `UpperBound`, and the documented status for interrupted input. Keep every existing assertion about other counters.

[Report type](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/inference/mod.rs#L130) · [Projection calculation](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/index/segment/projection/inference.rs#L283) · [Existing survivor oracle](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/index/segment/compaction.test.rs#L269)

## 5. Remove duplicate structural observations and release temporary memory

At the reviewed revision, `InferenceBuilder::observation()` calls `FieldState::observation_copy` before `finish_candidates`. It clones per-path Roaring bitmaps for presence, nulls, empty selections, terminal shapes and short-string coverage. `IndexedCollectionSegment` owns that copy. The collection-wide admission pass consumes counts and bounded distinct-key information, and the segment byte estimate omits the observation allocation. Optimize this specific data path. The wall-time savings have not been measured.

### 5.1 Compute the inputs that global admission actually consumes

1. Define a private compact field observation in `construction/inference.rs`. It holds the path, kind/shape flags, counts, capability and truncation flags, and the bounded distinct-key evidence needed to reproduce global admission. It must not contain a `FieldState`, posting dictionaries with slot sets, or per-field structural membership bitmaps. Keep the single policy decision in `decide_field_admission`. Candidate admission uses relaxed policy in `finish_candidates`; the collection must still use the configured policy.
2. Compute member-dependent facts **while the original sets are borrowed**, after applying ancestor contributions once and before consuming or dropping the sets. The following map identifies the overlap-sensitive operations in `AggregatedField::add_observation`. Take the remaining fields directly from its reads. Compute each local quantity once.

| Required global input | Exact segment-local evidence to keep |
| --- | --- |
| Present and empty-selection record counts | Cardinality of the union of field presence and its effective empty-selection set; separately retain the effective empty-selection cardinality. |
| Null and shape counts | Separate cardinalities of null occurrence, exact singleton null, empty array, scalar, array, object, direct and through-array sets. Also keep nonblank-record cardinality and array nesting. |
| Short-string coverage | `short_seen.len()`, `short_seen.difference_len(&short_uncovered)`, and the boolean result of the existing full required-domain test using presence, exact-null, seen and uncovered membership. Equal cardinalities alone do not prove set equality. |
| Occurrence counts and shape | Existing counters and their status from item 4; observed kinds, fractional-number and relevant shape flags. Preserve the evidence used for empty-string and length completeness. |
| Global thresholds | Distinct nonblank strings, distinct short keys, and distinct length keys or the existing truncation/overflow state. Two segments may contain the same key. Global distinctness requires a bounded union, even for segments with no final general-value dictionary. Preserve the existing distinction between an unavailable set and an empty one. |
| Physical accelerators | While the full candidate dictionaries still exist, capture whether they were present, complete or dropped. Continue obtaining actual retained index capability/size from the associated native posting where the existing aggregator requires it. |

3. Produce the optional compact observation and the native candidate state from one consuming finish path. Move bounded key collections and the prefix table where ownership permits. Where keys must be kept independently, retain only the bounded keys, never the posting values. Sort field observations by path and reuse that ordering for lookup, for example by binary search. The current aggregate repeats `.iter().find` over every field of every segment for each global path. Take advantage of the existing finish sort; this requires no new per-record data structure. Preserve externally visible deterministic ordering of reports and thresholds.
4. Keep the segment-local empty-ancestor prefix map only for the period where the collection must answer “a path seen in another segment was absent here; which local records reached an empty ancestor?” Several prefix bitmaps may contain the same slot. Union them within the logical domain before counting, including the root prefix from item 2. A sum of per-prefix counts loses overlap information. For a single-segment/local report without global aggregation, consume the prefix map during local finish and drop it.
5. Preserve the fast first-batch ownership transfer, later batch slot shifts and worker limits. Disabled inference and ordinary update paths that never participate in initial global admission must not construct a compact observation for later disposal. Keep timing and detailed work counters behind the existing profiling switches.

### 5.2 Bound the lifetime at publication

1. Remove the long-lived observation owner from `IndexedCollectionSegment`. Return a native candidate segment plus its optional compact observation from the initial-construction sealing path. Keep the existing plain native result for callers that need only a segment. Attach the optional observation by shared ownership to the **provisional** publication wrapper (`PublishedSegment`) so it travels with its segment ID. A provisional snapshot and a retried finalization can borrow the same immutable observation. Check all wrapper constructors and replacement/retirement paths.
2. `aggregate_segment_inference` must read the observation paired with each segment in the actual publication being finalized. Retain current conservative behavior for a missing/incomplete observation. Never pair observations and segments by unrelated vector order. Preserve atomic revision checks; failure or a stale revision must leave the previous provisional view valid for retry.
3. In `finish_initial_construction`, strip the observation when assembling the completed generation. If an observation is the **only** change, replace the small outer wrapper, reuse the existing `Arc<IndexedCollectionSegment>`, and clone the cheap shared `ActiveRecords` handle. The early continue for “no status refresh and no inference prune” must also clear provisional observation ownership. Where actual pruning or file-status changes already require updating the native payload, do both in that path. A reader holding a provisional snapshot may keep its observation until that reader releases it. A reader pinning the completed snapshot must keep zero construction observations alive.
4. Account for the compact observations while owned. Compute a cached byte **estimate** when producing each observation, using existing accounting conventions for owned path/key buffers, collection storage and the temporary prefix bitmaps. Charge it once per live observation in the provisional generation's memory accounting. Once finalization drops the provisional owner, the completed generation's staging charge is zero. Do not add it to each query posting, repeatedly walk bitmap members during publication, or label estimated bytes as process RSS. Use existing accounting and profiling surfaces where possible.
5. Preserve inference-report and admission parity across segment layouts. Test overlapping empty ancestors and a path seen only in a later segment, repeated short strings across segments, and distinct-length limits using the existing tests where present. Keep tests for null-only admission, opt-out, ReportOnly and bounded budgets. Compare meaningfully independent single-builder and segmented results, including counts and admitted capabilities. Exclude partition-dependent physical byte estimates from semantic equality.
6. Add a lifecycle regression giving direct evidence of release. Hold a provisional view, finish the build, and keep the completed view alive. Assert a positive provisional observation charge for a nonempty fixture and zero construction observations/bytes reachable from the completed view. For an unmodified native segment assert shared payload identity across publication (for example, `Arc::ptr_eq`). After dropping the provisional view and other temporary strong references, a weak observation reference must expire. Test Disabled inference has no observation. If the existing snapshot test harness can expose a stale-finalization retry, reuse it to verify that a failed attempt does not consume an observation needed by the successful attempt.

[Duplicated field state](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/construction/inference.rs#L890) · [Sealing](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/construction/segment_admission.rs#L908) · [Aggregation](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/construction/inference.rs#L218) · [Publication](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/lifecycle/publication.rs#L644) · [Segment accounting](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/crates/core/src/index/segment/mod.rs#L493)

## Validation, performance and handoff

Implement items 1–3 first and retain their regression fixtures while working on construction. Land the count contract before replacing the aggregation inputs. A targeted red/green run for each original bug, followed by one integrated validation, is sufficient. Follow the repository's existing `docs/ai` task/report conventions on the repair branch.

| Evidence | What to run or record |
| --- | --- |
| Public query and facet behavior | Actual baseline and repaired record IDs for the two query tables; both facet orders and their full coverage; selected plan or source-verification counters for the simple controls. Also run the existing null/type, wildcard, facet and lifecycle cases touched by the repair. |
| Diagnostic and memory contracts | Interrupted and repeated projection, complete-parent upper bound, exact scalar/zero controls, previous exact-null regression, and compact-observation parity/release tests. Record provisional estimated bytes, completed estimated staging bytes, and native payload reuse. |
| Integrated validation | Targeted tests via `cargo test -p objekte-search-core --lib --locked <filter>`. At the final code revision run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --locked` and `cargo test --workspace --locked`. Check the existing detailed-profiling build for touched instrumentation. Record failures or unavailable dependencies precisely. |
| Ordinary construction | Compare the repaired code with the pinned repair baseline, or the recorded actual pre-change tip if it already contains intervening changes. Use separately built release binaries, warm Musenalm ASCII, cache disabled, 8 requested workers, same resolved workers, corpus, schema, options, segment policy and host. Use the established `bench_index_construction` invocation in the linked report. Run three alternating fresh-process baseline/candidate pairs for **each** of inference Disabled and AutoIndex. If one comparison appears slower beyond ordinary pair scatter, run two additional pairs for that mode before interpreting it. Record raw wall times, paired differences and medians. Record peak/end RSS if available through the existing harness. |
| Interpretation | Attribute ordinary timing only to the uninstrumented runs. If available existing profiling can explain a regression, one separate instrumented pair for that mode is sufficient. A zero final staging charge verifies ownership independently of allocator RSS. Report overlapping timing distributions as inconclusive. No percentage speedup is an acceptance requirement. |

Build the ordinary executable once per revision with `cargo build --release --locked -p objekte-search-core --example bench_index_construction`. Run each fresh process with the appropriate revision's binary and a unique result directory. The second command enables AutoIndex. Use the same corpus location for both binaries.

```sh
"$benchmark_binary" --root "$musenalm_root" --result-dir "$disabled_result_dir" --alphabet ascii --workers 8
"$benchmark_binary" --root "$musenalm_root" --result-dir "$auto_index_result_dir" --alphabet ascii --workers 8 --inference
```

The results report should identify the implementation SHA, exact commands, pass counts and fixture outputs. Show before/after memory ownership, measured cost changes and any remaining unverified item. Acceptance requires all five work items, a reproducible public-API result for each original wrong answer, and evidence that initially published segments release construction observations without copying their native postings solely for cleanup.

[Prior comparable construction protocol](https://github.com/Simon-Martens/objekte/blob/aed69b4bc1b3/docs/ai/generated/projected_inference_and_construction_repair_results.md#L48) · [Full review baseline](https://github.com/Simon-Martens/objekte/tree/aed69b4bc1b3)

## Implementation report

Task identifier: `query-proof-facet-resolution-and-inference-repair`

Completion date: 2026-11-20

Base commit: `aed69b4bc1b3cb5d8bb97293d4cc1cdb394e96c7`

Final code commit: `3ece7c5daf`

Branch: `repair/path-posting-query-evaluation`

Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

The task directed continuation in the existing repair worktree, so no separate
integration worktree was created.

### Public query repairs

The reviewed revision returned only A for default `items.name = ""`; the
reference returned A and B. After the repair, both paths return A and B.
Existential `items.name ?= ""` returns A, and the indexed AND restriction
`items.name = "" && tag = "keep"` returns A and B. A simple scalar fixture still
uses exact empty-string evidence with zero source opens and parses.

The reviewed revision returned A and B for default `x:length = 1`; the
reference returned B. The repaired default and existential paths now return,
respectively:

- `x:length = 1`: B, E, F in the extended fixture.
- `x:length ?= 1`: A, B.
- `x:length != 1` and `x:length <= 0`: C, E, F.
- `x:length ?!= 1`: A, C.

Root empty arrays and arrays nested through root arrays now populate the
existing construction-only empty-ancestor table. A separately segmented empty
root record remains eligible for the default all-values comparison while the
segment containing the indexed length proof stays source-free.

### Segmented facets

Two one-record active segments were tested with the path-bearing segment in
both orders. Both pages return numeric key 7 with count 1, no continuation, two
eligible records, one present record, no explicit null, complete indexed
coverage, and no unresolved records. Finalized collection admission now
precedes selection of a compatible physical dictionary, so an active segment
missing the path cannot decide the facet result.

A finalized explicit opt-out remains non-enumerable even when a provisional
local posting is injected. A length-only field without a value dictionary also
remains unsupported.

### Occurrence diagnostics

Added public serializable `OccurrenceCountStatus` with `Exact`, `UpperBound`, and
`IncompleteObservation`. `InferredFieldReport` gains one
`occurrence_count_status` field applying jointly to all three occurrence totals;
Serde defaults missing fields to `IncompleteObservation`.
Adding this public field means external Rust struct literals for
`InferredFieldReport` must now specify it.

The interrupted fixture observes null, then stops, then leaves `["a","b"]`
unobserved. Its projected counts remain `(0,0,0)` with
`IncompleteObservation`, including after a second projection. A fresh
unrestricted observation of the survivor produces exact `(2,2,2)`. Empty
survivors are exact zeros. A fully counted parent projected to a strict subset
is `UpperBound`. Complete projected numeric coverage remains `Exact`.

### Compact observation ownership

`InferenceBuilder::observation` now constructs compact counts, capability flags,
bounded distinct keys, and the existing empty-ancestor prefix bitmaps. It no
longer clones `FieldState` posting maps, slot bitmaps, or structural field
membership sets into a second index owner.

The compact observation travels as an `Arc` on the provisional
`PublishedSegment`, paired directly with its segment identity. Collection
aggregation reads only that wrapper-owned observation. Completed finalization
clears the observation from the wrapper while retaining the same native
segment `Arc`.

Lifecycle evidence:

- Provisional compact observation estimate: 561 bytes.
- Completed-generation construction observation estimate: 0 bytes.
- Native segment payload identity was preserved with `Arc::ptr_eq`.
- A weak reference to the provisional observation expired after the provisional
  reader was released.
- Disabled inference produces no compact observation.

Existing inference parity tests passed for overlapping empty ancestors, paths
seen in later segments, repeated short strings, distinct-length limits,
null-only admission, opt-out, ReportOnly, and bounded budgets.

### Validation and performance

Commands and outcomes:

- `cargo test -p objekte-search-core --lib --locked`: 668 passed, 16 ignored.
- `cargo test --workspace --locked --no-fail-fast`: passed across CLI, desktop,
  core, integration tests, and doctests.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo check -p objekte-search-core --all-targets --features
  detailed-profiling --locked`: passed.
- `cargo clippy --workspace --all-targets --locked`: passed without errors and
  retained existing warning-level diagnostics.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Ordinary construction used separate release binaries, the warm 148,486-record
Musenalm ASCII corpus, eight workers, disabled verified cache, and three
alternating fresh-process pairs.

| Mode | Baseline samples | Candidate samples | Baseline median | Candidate median |
| --- | --- | --- | ---: | ---: |
| Inference disabled | 859.856, 891.520, 927.765 ms | 880.927, 896.718, 1003.711 ms | 891.520 ms | 896.718 ms |
| AutoIndex | 1532.829, 1563.542, 1578.795 ms | 1547.734, 1555.784, 1602.896 ms | 1563.542 ms | 1555.784 ms |

Both distributions overlap and the median differences are below 1%, so the
timing comparison is inconclusive. Median peak RSS was 568.5 MB baseline and
584.4 MB candidate without inference, and 914.0 MB baseline and 904.3 MB
candidate with AutoIndex. No percentage speedup is claimed.

No subordinate worktrees were created. The repair branch and worktree remain
for the owner's review and integration into `master`.
