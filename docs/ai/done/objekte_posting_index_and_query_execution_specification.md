# Objekte implementation specification — path postings and structural query evaluation

## 1. Assignment and completion condition

Implement the change in Objekte core, from construction through query execution. Each indexed path in a segment continues to own a PostingIndex. Retain three posting payloads: one presence bitmap, the existing ordered value dictionary, and the existing ordered array-length dictionary. Add a payload-free ScalarKey::Null to the value dictionary. Retire the dedicated per-path structural postings. Preserve the query language and the result of evaluating a query against the logical source record.

The purpose is to reduce the number of independently maintained record sets while keeping common queries file-free. On a fully covered logical segment, a count or record-ID query on explicit null or existence must be answerable from the index, even when the field has no ordinary scalar dictionary. String and number predicates should also be file-free wherever the remaining index proves them. A source read is an acceptable cost for a genuinely ambiguous selection. Preserve existing restrictions so that the reader verifies only the relevant candidates.

**Deliverable.** One reviewed integration branch with implementation, regression tests, and a short before/after report. The user merges it. Use task ID `path-posting-query-evaluation`, branch `integration/path-posting-query-evaluation`, and sibling worktree `../objekte-integration-path-posting-query-evaluation`. Read the repository instructions and follow `docs/ai/README.md` for the task document, temporary notes, commits, pushes, and completion report. Keep the original checkout on its existing branch. Start from the current authorized master tip and record the SHA. This specification was checked against [master at 32992e537e](https://github.com/Simon-Martens/objekte/tree/32992e537efa4c565bb18eb5301038c9db4d57de). Resolve moved symbols on your implementation base.

## 2. Storage contract

| Item | Required representation and meaning |
|---|---|
| Presence | A Roaring bitmap of segment-local slots for which the logical path selected **at least one terminal value**. Null, zero, false, empty strings, empty arrays and empty objects are all values. |
| Values | Keep the existing BTreeMap of typed ScalarKey to Roaring bitmap. Add one unit variant, Null. A slot belongs under Null when **any terminal value selected at this path is explicitly null**. Insert the key lazily on the first match. |
| Ordinary scalar keys | Preserve existing equality, ordering and array-element indexing. The empty string uses the ordinary String key. Id retains its existing variant. |
| Lengths | Keep the BTreeMap from numeric array length to Roaring bitmap. A slot can occur under several lengths when a path selects several arrays. Length counts immediate elements in each selected array. |
| Removed payloads | Remove PostingIndex.nulls, PostingIndex.empty_selections, PostingIndex.structural, StructuralFacts and StructuralKey. Eliminate the published inference side map of per-path empty selections after rewriting its callers. Keep no equivalent per-record posting of AnyNull, NonEmpty, String, Number, Array, Object or EmptyObject under another name. |
| Retained metadata | Preserve path, kind, multi, completeness/admission information, logical coverage, stats and segment identity. Retain the inference short-string coverage data needed to prove existing partial dictionary operations. This bitmap describes dictionary coverage. |
| Other indexes | The segment-wide empty_array_records used by text/gram candidate generation has a different contract. Preserve it. Preserve sort indexes, relation indexes, bigram indexes and the source evaluator. |

At a given path, `null`, `""`, `[]`, `{}` and an absent path have different meanings. Do not introduce an “empty” key combining them. Do not count `[null]` as a null at the path selecting that array. Preserve the existing rule that a null occurrence can coexist with another value in the same record.

### Required proof metadata

Keep coverage of a particular fact distinct from whether its bitmap happens to be empty. A path missing its Null key proves zero matching slots **only if its binding and segment guarantee full observation of selected nulls**. General scalar value completeness is independent. For example, an unindexed declared text field can have complete null coverage while its nonempty strings were never inserted. An incomplete inferred path or a custom extractor cannot claim that guarantee merely because its dictionary is empty.

Use a small, fixed-size optional summary on PostingIndex for source-backed logical selections. Suggested Rust name: `SelectionSummary`. Its presence certifies complete terminal observation for that path and logical domain, including null and presence. Store a bit mask of the terminal value kinds encountered, plus one boolean indicating whether any record selected more than one terminal occurrence. A u16 mask is sufficient for the current value variants. Populate it in the existing observation pass, before expansion. This metadata is per **path and segment**, independent of record count. It stores no slot IDs. Use it to distinguish an observed scalar-only domain from an array-valued path that emits scalar dictionary entries. An unknown summary disables proofs requiring it. A schema hint, an inferred representative kind, `multi` alone, or the first B-tree key is insufficient. Named extractor bindings may describe their own outputs but must not certify original field semantics without an existing equivalence contract.

For disjoint, fully observed batches merge the masks with OR and the multiplicity flag with OR. Never promote a partially observed input to complete. A projected subset can inherit the parent's possible-kind mask and multiplicity as conservative upper bounds. Recompute exact slot sets through projection. A stale possible kind may prevent an optimization but must never exclude a live match. Keep these upper bounds separate from public counts. Prefer existing equivalent metadata when it already satisfies these invariants.

## 3. Source truth and examples

The source evaluator applies each state predicate to terminal occurrences and takes “any”. It then applies the user's Boolean comparison to that one result. Absence and a successful traversal yielding zero terminal values are both false for every positive state predicate. They can differ for universal comparisons and paired field comparisons. Keep that distinction in source evaluation. [State evaluator](https://github.com/Simon-Martens/objekte/blob/32992e537e/crates/core/src/query/semantics/state/mod.rs)

The following table applies to path `x`. “Content” means the existing `:?` predicate. Array lengths here refer to stored length keys when that capability is enabled.

| Record | Present | Null key | String | Number | Content | Array lengths |
|---|---|---|---|---|---|---|
| `{}` | no | no | no | no | no | — |
| `{"x":null}` | yes | yes | no | no | no | — |
| `{"x":""}` | yes | no | yes | no | no | — |
| `{"x":"Ada"}` | yes | no | yes | no | yes | — |
| `{"x":0}` | yes | no | no | yes | yes | — |
| `{"x":false}` | yes | no | no | no | yes | — |
| `{"x":[]}` | yes | no | no | no | no | 0 |
| `{"x":[null]}` | yes | no | no | no | yes | 1 |
| `{"x":[""]}` | yes | no | no | no | yes | 1 |
| `{"x":{}}` | yes | no | no | no | no | — |
| `{"x":{"a":null}}` | yes | no | no | no | yes | — |

For `{"authors":[{"name":null},{"name":"Ada"}]}`, the path `authors.name` sets presence, Null and the ordinary string key for Ada when value indexing is enabled. Both `:null` and `:string` are true. For `{"authors":[]}`, the path `authors` is present with length zero. The path `authors.name` is absent under the new presence definition. An empty terminal array and a traversal yielding zero values must have separate fixtures.

## 4. Implement construction first

1. In `crates/core/src/index/posting/mod.rs`, append Null to ScalarKey to preserve the existing variant sequence. Keep conversion policies explicit. The shared `ScalarKey::from_value` currently omits null and is called for flattened elements, literals and unrelated lookups. Leave its ordinary-scalar contract intact and insert ScalarKey::Null explicitly in the terminal-selection builder. Document this on the variant or conversion function.
2. In `PostingIndex::add_values`, account for the covered logical record once, including an absent path. If selection is missing or empty, finish without setting presence or inserting keys. Otherwise set presence once. For each terminal occurrence update the summary, record null under Null, then process ordinary scalars and arrays under the existing policies. A null occurrence must not terminate processing of later siblings. On a selected array, record its length when enabled, and send elements through the existing ordinary-value admission rules. Keep the existing fast single-value path and allocation reuse where possible. Never materialize a vector just to test for null.
3. A source-backed declared path that previously supported structural null lookup retains that coverage even with `indexed: false`. Preserve existing opt-out behavior for general values, length and empty strings in PostingCollection::new. Keep named value index construction distinct. Preserve declared, inferred, explicitly prohibited, report-only and disabled admission decisions. Do not admit a new path solely to answer this query.
4. Update stats, validation and allocation estimates together. Presence count is the new presence cardinality. An internal count named “selected null” means cardinality of the Null entry. Keep ordinary distinct-value count and ordinary min/max free of Null for consumers using them to cost scalar operations, sort, or enumerate facets. Handle null-only maps and empty maps. Verify every posted slot is eligible and in bounds, selected null is a subset of presence, and coverage checks use the logical domain. Charge a populated Null posting once in retained memory and admission budgets. Remove charges and clones for retired structures.
5. **Inference has its own builder.** In `construction/inference.rs`, record selected null only in the terminal branch of `visit`. The ordinary `add_scalar` sees array elements too. Its `null_occurrence` includes element nulls. Its `exact_null` means exactly one selected null. Neither is interchangeable with the new Null entry. Move the terminal-null bitmap into the final values dictionary when publishing an admitted complete path. Retain it across loss of the general dictionary if terminal coverage remains complete. If a budget or premature termination discarded terminal evidence, withhold the capability. Build the terminal summary even on a null-only observed path whenever admission rules publish it.
6. In inference finalization, take the new presence from actual terminal visits. The existing `field.present |= empty` step adds records reached only through an empty ancestor. Keep any temporary diagnostic/accounting logic away from published presence. Remove old provisional collections serving solely the deleted facts. Keep construction-only observations needed for admission, compatibility statistics and the short-string coverage calculation. Never use “any selected null” to exempt an entire record from short-string coverage: one record may also contain a long unindexed string.
7. Check the alternate inference aggregation, worker batch merge, provisional-drop and scan-built inferred-segment paths. They must make the same coverage claims. In particular do not leave a cleared bitmap behind with a “complete” capability. [Declared construction](https://github.com/Simon-Martens/objekte/blob/32992e537e/crates/core/src/index/posting/mod.rs) · [Inferred construction](https://github.com/Simon-Martens/objekte/blob/32992e537e/crates/core/src/construction/inference.rs)

## 5. Query access and exactness

Put predicate classification and posting selection inside the native posting access implementation. Use the same classification for cost estimation, unrestricted exact execution, and execution restricted by a previous term. Keep dictionary internals out of the generic planner and runner. Use the existing proposal and exact/candidate evidence types. An unsupported lookup returns “unavailable”. An exact empty set represents a proved result.

Use the following notation. D is the effective input intersected with the segment's live, eligible logical slots. P is terminal presence. N is the selected-null posting, borrowed if present. U is the union of selected dictionary entries for a predicate. All complements are relative to D. Assess coverage against the full eligible segment even when D has already been narrowed. The existing fallback route separately handles nonlogical records. Preserve that routing, selector checks, transform checks and named-binding checks.

| Query | Execution when certified |
|---|---|
| `x:exists = true`, `x:exists = false` | D ∩ P, D − P. A successful zero-value traversal adds no slot. |
| `x:null = true`, `x:null = false` | D ∩ N, D − N. The negative state includes missing paths. No source read on a covered logical input. |
| `x = null`, `x != null` | For a supported scalar selection with guaranteed single occurrence where the evaluator gives ordinary scalar comparison, D ∩ N and (D ∩ P) − N. Preserve existing quantifier, path and empty-traversal restrictions. A missing scalar field fails literal inequality. For element/universal/paired cases, follow the source comparison or decline the proof. |
| `x:length` | Retain the numeric equality, range and quantifier rules of `length_posting_is_exact`. A length posting proves the existence of a selected array of that length. A nested default/universal comparison may need verification even with complete lengths. A record can appear in both lengths[0] and lengths[2]. |
| `x:string`, `x:number` | Apply the algorithm in §6. String includes Id. Number matches only the Number variant, including the engine's exact decimal representation when present. Unixtime, Bool and dates retain their own semantics. |
| `x:array` | When all selected array lengths are covered, the union of all length postings is an exact positive state set. Prefer a cheaper certified single-kind proof if available. |
| `x:object` | A complete shape summary excluding objects proves no matches. A selection guaranteed to contain a single object or null when present permits P − N. Otherwise use verification when the available facts cannot distinguish the records. |
| `x:?` | For scalar-only single-occurrence number or Bool paths, P − N is exact. For a single string or null, also subtract the complete selected empty-string posting. Account for empty Id separately when relevant. For a path selecting only arrays or null, union lengths greater than zero if complete. In all other ambiguous cases use safe restriction and verification. Preserve source semantics for whitespace, false, zero, empty objects and nonempty containers holding null. |

For Boolean state predicates, apply the existing normalization for `= true`, `= false`, `!= true`, and `!= false`. Invert the aggregated record result. For mixed selections [null, “Ada”], subtracting N from P discards a valid string match. For ["", “Ada”], subtracting empty-string slots discards a valid content match. Restrict subtraction formulas to the stated single-occurrence proof. Use ordinary per-key unions for eligible existential cases.

### Empty traversal, polarity and planner fallbacks

- Remove the existing broad `SelfComparePresence` shortcut or narrow it to a provably equivalent case. Implement a narrow fast path for a direct, single field step with all the existing scalar identity guards, proven scalar-only selections, and at most one terminal occurrence. A direct missing field then has the evaluator's missing behavior. For paths through arrays that can yield zero terminal values, leave the paired comparison to the source evaluator. Include a differential test that distinguishes missing from successful empty traversal.
- `InferenceState.empty_selections` also protects some **universal value candidates**. It is different from the old field bitmap. When dropping that side map, withdraw candidate offers requiring it unless their superset follows independently. Preserve ordinary existential offers. Never replace the old exception set mechanically with D − P: a present terminal empty array can also yield an empty element stream.
- For an ambiguous **positive state** that implies presence, D ∩ P is always a safe candidate set under verified presence coverage. A complete but flattened type-key union can improve that restriction. For the negative of an approximate result, do not subtract the candidate superset. Use existing evidence handling if it can preserve proved cases and verify unresolved slots. Otherwise pass the incoming domain through to the residual evaluator. For partial dictionaries, only drop slots under an independent proof, e.g. a certified covered subset with its uncovered remainder added back.
- Preserve existing evidence handoff for AND, OR, NOT, short-circuit, restricted input and nested disjunction. Exact evidence can discharge that predicate in the proved domain. A candidate still needs the source check. Refuse positional/explicit selector rewrites unless both terminal selections coincide. A trailing explicit element step can select an element null that the container path never selects. Preserve index use for certified equivalent declared traversals such as an array of objects followed by a field. Custom extractors, metadata roots and raw fallback records keep their own evaluation rules.
- Revisit ordinary scalar inequality, temporal comparisons, inferred value candidates, string contains and sort admissibility. Replace every old dependency on singleton null, raw presence or per-path empty selections by an equivalent proof or withdraw the fast path. An operation that iterates only string keys must skip the new Null variant without failing. Null must never leak across numeric or temporal range bounds. [Posting lookup](https://github.com/Simon-Martens/objekte/blob/32992e537e/crates/core/src/index/posting/lookup.rs) · [Proposal and execution](https://github.com/Simon-Martens/objekte/blob/32992e537e/crates/core/src/query_access/catalog.rs)

## 6. Type planning algorithm

Implement this for positive type tests first, then apply Boolean polarity to exact answers. The dictionary stores **values → slots**. Determining a type from a key is cheap, but building the result for thousands of distinct keys still touches thousands of postings.

1. **Check selection identity and coverage.** Confirm the index represents the same logical path, traversal and binding as the term. Check summary and dictionary completeness separately. A schema type hint alone need not constrain every selected occurrence. Summary evidence can offer an exact type answer even on a path with partial ordinary values.
2. **Use a constant-work proof first.** If the complete possible-kind mask excludes the tested kind, the positive result is empty. If every possible selected kind satisfies the type, it is D ∩ P. If every possible non-null kind satisfies the type and at most one occurrence can be selected per record, it is (D ∩ P) − N. A null-only segment has an empty positive result for both string and number. For the string test, treat String and Id together in each proof. For number, include only Number.
3. **Use a typed range when terminal scalar keys are complete.** If every selected value of the requested type is represented, and matching keys can only have come from terminal occurrences, take the union of all matching-key postings within D. Scalar paths traversing several array elements are eligible: several author names are still terminal strings. A conservative certificate for this implementation is complete ordinary values for the bound path and a complete terminal summary excluding terminal arrays. Under partial ordinary indexing, only use this exact route with an existing independent proof of complete coverage for the requested type. A complete empty-string shortcut does not cover all strings.
4. **Bound dictionary traversal.** Use borrowed B-tree ranges, without string clones per key or a temporary list of keys. In the inspected enum order, String and Id are contiguous: seek forward from String(String::new()) and stop on the first other variant. Number immediately precedes Bool: seek backwards from the exclusive Bool(false) bound and stop at the first non-Number. This covers the engine's entire numeric domain without inventing a minimum integer or float sentinel. Encapsulate the bounds next to ScalarKey, keep Null outside these runs, and test first/last and neighboring variants. If the enum order on the implementation base differs, derive the bounds from that order and test them.
5. **Use a candidate range only when it covers every possible match.** When terminal arrays contributed ordinary element keys, a string-key hit could mean either `"Ada"` or `["Ada"]`. A full string-key union is still a safe positive candidate under complete target-value coverage. Verify survivors. If short-string indexing omitted long strings, include the certified uncovered part of D, or use D ∩ P and verify it. If the index cannot prove a useful restriction, return unavailable. Preserve the original candidate restriction for negative tests unless the answer is exact or the engine explicitly represents the two evidence domains.
6. **Separate costing from execution.** A direct Null or presence probe takes constant lookup work. Seek and probe type ranges using the existing bounded-probe convention (up to 33 keys in the inspected implementation). Estimate bitmap visits and output, cap estimated output at the input cardinality, and account for overlap. Never allocate the union in planning or report the sum of overlapping bitmap lengths as an exact result. Exactness is independent of cost. Execution should borrow postings and stream unions into a result bitmap in the effective domain. Where the existing access contracts allow a borrowed direct posting, keep it borrowed. Avoid cloning whole postings just to narrow them. Do not materialize a type bitmap as a cache in PostingIndex.

A covered, single-valued year field with 100,000 distinct numbers therefore takes one presence/null calculation. A mixed scalar field with two distinct numeric keys can use two postings. A record with a terminal array of numbers can require source verification for a type test. These are three distinct plans; exercise each.

## 7. Segment, lifecycle and public-consumer changes

Use this table to divide the work. Audit the complete workspace on your branch, including examples and tests. Reaching a clean compile catches exhaustive matches but misses code that silently accepts Null through a wildcard.

| Implementation area | Required work |
|---|---|
| `crates/core/src/index/posting/{mod,lookup}.rs` | Implement construction, borrowed null access, capability checks, exact sets, restricted sets and planner estimates. Preserve scalar equality, temporal probes, dictionary contains and length paths. Unify duplicated eligibility decisions across entry points. |
| `crates/core/src/query_access/{catalog,segment}.rs` | Replace state-source selection, self-comparison, short-string coverage proofs, candidate selection and sortable-scalar eligibility. Preserve the exact/candidate contract across the segment's active logical and fallback domains. |
| `crates/core/src/construction/inference.rs`, `inference/mod.rs`, `lifecycle/publication.rs` | Update provisional observations, admission evidence, byte estimates, diagnostics, aggregation and opt-out cleanup. A short-string coverage check that iterates ordinary dictionary entries must exclude the internal Null key. Replace the retired structural-capability indicator in internal reporting with accurate supported capabilities. For externally consumed diagnostics, preserve compatibility and document any retired field. |
| `crates/core/src/index/segment/projection/{postings,inference}.rs` | Update empty projected postings, partition scattering, individual projection, projection of inference state and report recomputation. Remap Null through the ordinary value-key loop exactly once. Preserve complete null coverage for partitions with zero nulls. |
| `crates/core/src/index/segment/{consolidation,compaction,mod}.rs`, `lifecycle/mod.rs` | Update clearing, merging, slot remapping, retained-size accounting and validation. Respect checked slot shifts, active tombstones, record revisions and view pinning. Merge distinct coverage conservatively. A missing proof in a contributing segment disables that proof in the merged index or leaves it to segment-local planning. |
| `crates/core/src/repository/posting_values.rs` | Exercise collection and segmented variants, including the inferred fallback builder. Keep public ordinary value lists, counts, pagination, cursors, prefix and bounds stable. Null is an internal lookup key in this assignment. Exclude it **before** limits and continuation decisions. Keep the existing explicit-null coverage statistic as a distinct calculation. |
| Sorting, facet consumers, relation postings, CLI and desktop | Audit key cursors, first/last/min/max, sort null placement, string-only loops, and public ScalarKey matches. Make minimal compile/behavior adaptations in CLI and desktop where required. Preserve ordinary autocomplete values, ordering, ID matching, and existing null/missing sort behavior. |
| Active format boundaries | ScalarKey already derives Serialize and Deserialize. Audit live codecs, exported keys and format/version contracts on the implementation branch. Preserve existing variant tags and old-key decoding. If this branch actually persists field postings, version the changed payload and use its supported migration or rebuild behavior. An old bitmap with singleton-null semantics cannot certify new mixed-occurrence membership. Keep unavailable evidence unknown until rebuilt. Do not bring an abandoned snapshot prototype into this assignment. |

### Public counts that the three posting payloads cannot always reproduce

At the reference revision, `PostingValuesCoverage.explicit_null_records` uses the old **singleton-null** bitmap; ordinary facet rows exclude null. A record selecting both null and “Ada”, or selecting two nulls, belongs in the new Null posting but contributes zero to that old statistic. Likewise legacy `present_records` in the posting-values API can include a successful traversal with no terminal value. Keep the query's new presence definition and the public statistics' established meanings separately documented.

For unrestricted populations, preserve cheap scalar counts during the existing build where useful. A zero legacy empty-traversal count also certifies that P answers legacy presence for every subset of that same population. A zero count of non-singleton null selections likewise certifies that N answers singleton-null counts for any subset. These are inexpensive ways to avoid facet reads on ordinary paths. After projection an inherited zero remains a proof; an inherited nonzero count is only an upper bound unless recomputed. On a requested subset, use the new bitmap directly when equivalence is proved, e.g. singleton null on a path selecting at most one value. Otherwise verify only the ambiguous portion: only N can contain singleton-null records, and only the requested slots outside P can add legacy zero-value traversals. With a custom binding, use its existing extraction semantics. If a count is already available for precisely the requested population, reuse it. Keep any exceptional verification inside this reporting API. The ordinary null query must never incur those reads. Do not restore a retained singleton-null bitmap or per-path empty-traversal bitmap to accelerate this statistic.

Inference reports also have separate `exact_null_records`, `null_occurrence_records` and `empty_selection_records`. Preserve documented meanings, keep build-time counts where needed, and respect existing report completeness behavior during projection. Never fill a singleton-null or flattened-null count from `values[Null].len()` solely because the old code read `index.nulls.len()`. For active APIs promising exact counts after compaction or restriction, reproduce them with valid aggregates for the same domain or targeted evaluation. No false exact count and no source pass across the corpus during publication to reconstruct retired bitmaps. If the existing diagnostic contract permits unavailable counts, report that state explicitly.

Run this audit from the worktree root, then inspect the remaining references individually:

```sh
rg -n 'StructuralFacts|StructuralKey|empty_selections|empty_only|selected_null|exact_null|\.nulls\b' crates
rg -n 'ScalarKey::|from_value\(|for_each_value|value_range\(|distinct_values\(|min_key|max_key' crates
rg -n 'structural_facts|empty_string_complete|length_complete|short_string_coverage|SelfComparePresence' crates
```

## 8. Verification plan

**Oracle.** Reuse the source/reference evaluator independently of the new posting helpers. Compare matched IDs, counts, ordering and errors for unsupported/ambiguous expressions. Exercise the normal public search entry point; pure lookup tests alone cannot show that the planner selects the new offer. Run query tests with the verified-result cache disabled. Use count/ID-only results to separate predicate reads from reads requested for displaying source fields.

| Test group | Required cases and assertions |
|---|---|
| Construction truth table | All rows in §3, multiple selected values, duplicate nulls, and a mixed record with an empty branch and a populated sibling. Assert raw membership of P, Null and lengths, as well as the absence of an empty Null key. For `{"x":[null]}`, assert length 1 and no Null key for x. |
| Positive and negative Boolean state | Exercise both Boolean literals and both supported equality operators for exists/null/type. Missing path: `:null = false` succeeds, single-valued literal `!= null` fails. Include a missing path, selected empty array, and empty traversal in the same segment. |
| Typed proofs | String plus Id, Number variants supported by the model, numeric-looking strings, zero, false, dates/unixtime, overlapping postings, a null-only segment, and high-cardinality homogeneous scalars. Exact count/ID queries on complete typed fixtures must show zero opens/parses. Confirm the homogeneous high-cardinality case avoids visiting every distinct key. |
| Multivalued and ambiguous types | Test `{"x":"Ada"}` beside `{"x":["Ada"]}` and `{"x":7}` beside `{"x":[7]}` on a reader/schema configuration admitting both. Test multiple terminal names, including [null, “Ada”]. Verify that an ambiguous candidate is never promoted to exact or complemented as exact. Include a second selective term and prove source verification respects its incoming slots. |
| Other predicates | Preserve `:array`, `:object`, `:?`, array length equality/ranges, null literals, ordinary string contains, same-field comparisons and negative forms. Include an empty object beside an empty string and a nonempty object; a multivalued path selecting arrays of lengths 0 and 2; terminal vs intermediate explicit wildcards; positional and default/universal cases with zero selected values. Preserve source errors when applicable. |
| Admission and bindings | Run declared indexed, declared without general values, inferred full, short-string partial with an unindexed long string, report-only, disabled and named extractor cases. Include a custom binding whose output differs from its source and one budget-dropped path. Construct a segment that completely indexed nulls and found none, and one without the capability. Only the former may prove an empty null result from a missing key. |
| Execution composition | Compare restricted and unrestricted execution, AND/OR/NOT including nested OR, and a paginated or streamed query. Exercise both the access decision and residual evaluation. Test a mixed logical/raw-fallback domain and multiple segments with different path coverage. Inactive records and out-of-domain slots must never appear. |
| Lifecycle | Compare original build to partitioned/parallel build, then project or consolidate, retire/update records, compact, and query a pinned old view and a new view. Include a null record in a nonzero-offset segment, a formerly last null that is retired, and a segment with no occurrence of the field. Assert reference parity, coverage and active slot membership at each relevant stage. |
| Public consumers | Facet pagination with limit 1 in both directions on null-only and mixed dictionaries; prefix, bounds, filtered coverage, mixed null/ordinary selections and two nulls; sort null/missing on both orders; ordinary string contains on a map containing Null. Assert public counts and pages against the pre-change behavior. Include any active codec compatibility identified in §7. |

For exact covered exists/null queries include **zero files opened, zero record parses and zero scanned records for predicate evaluation** in integration assertions or existing instrumentation. Test a declared field with ordinary indexing disabled and an inferred path admitted with selected-null capability. Use a cold result cache and fully logical test records. Also test a real fallback so that a broken read counter cannot make the zero-I/O assertion pass accidentally.

### Performance evidence

Compare release builds of the recorded parent and final integration revision on the same Musenalm corpus, configuration, worker count and result mode. Record at least three comparable timed runs for changed paths and report medians. Separate construction from query timing. Use the extended manifest; inspect IDs on the actual branch. In the reference revision Q116 is `contributors:length > 0` and Q117 is `contributors:length = 0`.

- Run `verify_structural_reference` using the existing local corpus root and selected cases. Example: `cargo run --release -p objekte-search-core --example verify_structural_reference -- "$MUSENALM_ROOT" Q106 Q107 Q116 Q117 Q120 Q195 Q196 Q198 Q203 Q207 Q208 Q218 Q219 Q220`. This program checks source parity and prints file opens, bytes, parses and scans. Inspect the manifest's return mode before interpreting counters. Use the existing timing harness for latency.
- Record construction time, peak or retained memory using the same metric on both revisions, and retained posting bytes or bitmap counts from the existing accounting. Report typed-key visits, source opens/parses and candidate counts where available. Compare direct null, length, homogeneous type, mixed scalar type and an intentionally ambiguous fallback. Include one scalar range, ordinary text query and sort/facet smoke case to catch accidental spillover.
- A latency change within observed run-to-run variation is inconclusive. A previously file-free common single-valued string/number query that now opens most files is a regression to fix. If a multivalued structural query newly scans a substantial part of the corpus, give its actual before/after time, verification count, and reason. Try the safe cheap proof or narrower candidate in §§5–6 first. Report the remaining cost to the user rather than restoring per-record type facts or hiding the result in an average.
- Run the available workspace checks, including `cargo fmt --all -- --check`, `cargo check --workspace --all-targets` and `cargo test --workspace`, plus the relevant release-mode examples/integration tests. If a real corpus or toolchain dependency is missing, record precisely which validation is pending, and still complete synthetic end-to-end tests.

## 9. Implementation sequence and sign-off

1. In the integration worktree, record baseline revisions/test results and capture representative query outcomes, facet counts and counters **before** editing. Add focused regression fixtures for ambiguous cases where existing tests give no independent oracle.
2. Implement terminal observation, Null-key insertion, and new presence in both declared and inferred builders. Carry the representation through segment construction and projection. Build-time compatibility scaffolding may be used locally; remove it by the final commit.
3. Switch the query access layer and costing together. First implement exists/null/length. Then implement the single-kind proof, typed-range union, and fallback rules. Verify source parity and I/O at the public entry point after each group.
4. Finish public consumer, diagnostics, consolidation, compaction and memory accounting changes. Remove obsolete payloads, query proposals and side maps. Search for old assumptions, including uses of `values.is_empty()` as evidence that a field has no ordinary values. Complete the ordinary-key and sort/facet audit.
5. Run the verification plan. Attach a report under `docs/ai/generated/path-posting-query-evaluation/` with the baseline/final SHAs, changes by subsystem, validation commands and outcomes, measurement conditions, before/after results, and any remaining source-verification cases. Complete the repository's task lifecycle on the integration branch.

**Ready for review** means the physical per-path payload comprises presence, values and lengths; selected explicit null is indexed once under ScalarKey::Null; existing query forms agree with the source evaluator; the tests demonstrate which lookups avoid file reads; and the report identifies the actual cost of the cases that still require verification.

## Implementation report

- Task identifier: `path-posting-query-evaluation`
- Completion date: 2026-11-20
- Base commit: `32992e537efa4c565bb18eb5301038c9db4d57de`
- Integration branch: `integration/path-posting-query-evaluation`
- Integration worktree: `/home/simon/source/objekte-integration-path-posting-query-evaluation`

### Delivered behavior

The per-path posting representation now consists of terminal presence, one
`ScalarKey::Null` entry in the ordered value dictionary, ordered ordinary value
keys, and ordered array lengths. The dedicated null, empty-selection, and
structural fact payloads and the published inference empty-selection side map
were removed. `SelectionSummary` retains only a fixed kind mask, multiplicity,
and successful-empty-traversal state for complete terminal observation.

Declared and inferred construction now insert explicit terminal null once,
preserve null evidence independently of ordinary value admission, and keep
zero-value traversals distinct from terminal presence. Projection, compaction,
consolidation, worker-batch merging, slot remapping, memory estimates, sorting,
facets, and diagnostics carry the new representation. Public facet statistics
retain their previous singleton-null and empty-traversal meanings; only the
ambiguous portion of a requested population is source-verified.

Query access now classifies exists, null, type, content, length, scalar, and
ambiguous state predicates in the posting implementation. Fully covered
presence and selected-null probes are exact, homogeneous type domains use
constant-work masks, bounded typed ranges are used where the dictionary proves
them, and multivalued/partial cases return safe candidates or fall back to the
source evaluator. Negative approximate state answers are not complemented.
The broad self-comparison shortcut now applies only to a direct, single scalar
field. Successful empty traversals force verification when universal
comparison semantics could differ from a missing path.

For compatibility, the existing external diagnostic field named
`structural_facts` is retained as an alias for complete terminal-summary
coverage. Inspection retains `declared:<path>:nulls` and
`inferred:<path>:nulls`; the retired per-path `:empty` and `:structural`
components are no longer emitted.

### Changes by subsystem

- `index/posting`: evolved `ScalarKey`, terminal presence, lazy `Null` entries,
  fixed summaries, capability-aware validation, stats, typed ranges, exact
  state sets, and candidate sets.
- `construction/inference`: terminal observation, null-only admission,
  selected-null retention, empty-traversal summary, aggregation, publication,
  and projection-safe reports.
- `index/segment` and lifecycle: projection/scattering, consolidation,
  compaction, clearing, merging, accounting, and publication cleanup.
- `query_access` and execution: exact/candidate classification, restricted
  execution, type/content proofs, and narrowed self-comparison.
- `repository/posting_values` and consumers: hidden `Null` dictionary key,
  preserved public counts, facet continuation behavior, inspection, sort, CLI,
  and desktop handling.

### Contributing commits

- `dbfd2aa197` — Consolidate path postings around terminal selections
- `12e8e048d6` — Refine terminal summary proving and null admission

No subtask branches were created.

### Validation

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets` — passed.
- `cargo test --workspace` — passed. Core: 637 passed, 16 ignored; all CLI,
  desktop, integration, and documentation tests passed.
- `verify_structural_reference` on the final release binary for Q106, Q107,
  Q116, Q117, Q120, Q195, Q196, Q198, Q203, Q207, Q208, Q218, Q219, and Q220
  matched the source reference. Every case reported zero files opened, zero
  bytes read, zero record parses, and zero scanned records.
- Synthetic public tests cover the complete selection truth table, mixed
  null/string occurrence, duplicate nulls, empty strings, empty arrays,
  missing paths, successful empty traversal, null-only inferred paths,
  homogeneous type probes, String/Id matching, the empty-string content
  shortcut, multivalued candidate verification, public facet counts, and
  lifecycle parity.

### Performance evidence

Release binaries were built from the recorded base and final revisions and run
against the same local Musenalm corpus (148,486 records), ASCII bigrams,
four query workers, verified cache disabled, and the extended manifest. The
base query timings below use three repetitions for Q106/Q116/Q117/Q120 and nine
repetitions for the remaining cases; the final replay likewise uses nine
repetitions for those remaining cases. Medians are in microseconds.

| Case | Query | Base median | Final median | Change |
| --- | --- | ---: | ---: | ---: |
| Q106 | `data:null = true` | 47.659 | 54.323 | +13.9% |
| Q116 | `contributors:length > 0` | 215.716 | 215.436 | -0.1% |
| Q117 | `contributors:length = 0` | 204.408 | 207.821 | +1.7% |
| Q120 | `incipit_statement = ""` | 97.273 | 101.479 | +4.3% |
| Q195 | `annotation:? = true` | 24.382 | 32.030 | +31.4% |
| Q196 | `annotation:string = true` | 35.133 | 34.383 | -2.1% |
| Q198 | `year:number = true` | 22.934 | 22.507 | -1.9% |
| Q203 | `contributors[*].type:null = true` | 59.145 | 56.974 | -3.7% |
| Q207 | `contributors.type:string = true` | 205.607 | 196.968 | -4.2% |
| Q208 | `contributors.type:null = true` | 56.130 | 54.771 | -2.4% |
| Q218 | `contributors.type:string = true && contributors.type:? = true` | 514.080 | 492.285 | -4.2% |
| Q219 | `year:number = true && year >= 1800` | 168.615 | 168.818 | +0.1% |
| Q220 | `contributors.type:string = false` | 195.275 | 199.248 | +2.0% |

All measured cases remained file-free. Q195 increased by about 7.6 µs while
still opening no source file; it replaces the retired direct `NonEmpty`
posting with a typed/content proof. The small wall-time changes in the other
microsecond-scale cases are below meaningful application latency and do not
represent source-I/O regressions.

Fresh ASCII construction measurements were:

| Metric | Base | Final |
| --- | ---: | ---: |
| Build time, three samples | 0.737, 0.752, 0.771 s | 0.688, 0.750, 0.675 s |
| Median build time | 0.752 s | 0.688 s |
| Resident bytes | 697,671,680 | 645,255,168 |
| Peak resident bytes | 739,840,000 | 689,524,736 |
| Estimated index bytes | 111,249,432 | 104,345,704 |

The final construction median improved by 8.5%, retained memory decreased by
52.4 MB, peak resident memory decreased by 50.3 MB, and estimated index bytes
decreased by 6.2%. Record parses remained 148,486 on both revisions.

Representative commands:

```sh
cargo build --release -p objekte-search-core \
  --example bench_musenalm --example verify_structural_reference

target/release/examples/verify_structural_reference examples/musenalm \
  Q106 Q107 Q116 Q117 Q120 Q195 Q196 Q198 Q203 Q207 Q208 Q218 Q219 Q220

target/release/examples/bench_musenalm --root examples/musenalm --extended \
  --cases Q195,Q196,Q198,Q203,Q207,Q208,Q218,Q219,Q220 --profiles ascii-off \
  --repetitions 9 --verified-cache-bytes 0 --query-workers 4
```

### Remaining limitations

- Multivalued paths whose typed dictionary may contain array-element keys use a
  typed candidate restriction followed by source verification. This is
  intentional; those candidates are never promoted to exact or complemented
  as exact.
- Mixed object paths cannot prove `:?` or object positive/negative membership
  from the fixed summary unless the observation reduces to object/null with a
  single occurrence. They retain safe source verification.
- Projected inference reports mark per-field completeness false when the
  summary may contain multiple terminal occurrences or an empty traversal, so
  they do not present inherited upper bounds as exact counts.

### Cleanup

No subordinate worktrees were created. The integration branch and worktree are
retained for user review.
