# Agent specification: query scanner plans and posting hot-path optimization

Task: `hybrid-index-storage` / query optimization subproject
Repository: `Simon-Martens/objekte`
Plan date: 14 November 2026
Base revision for the review: `471b79c35bb67a2b905ba24e2d6ad5b694c20fe4` on `integration/hybrid-index-storage`
Latest query evidence: `bench/results/hybrid-index-storage/repair-20261113/final-e15b` (no newer campaign)

## 1. Objective and scope

Implement and measure five query-path improvements, in this order:

1. **Structural predicate indexing and routing** — make `:array`, `:object`, `:string`, `:number`, `:null`, `:?`, and `:exists` answerable from structural postings for declared fields, including fields that are not value-indexed, and allow occurrence-existential state predicates with explicit array steps. (Q202, Q203, Q204; foundation for Q187.)
2. **Self-comparison rewrite** — detect a field compared with itself (Q187: `contributors[*].type = contributors[*].type`) and reduce it to an exact presence predicate, proven by differential tests.
3. **Scanner allocation and evidence reduction** — cut per-record allocations and unnecessary evidence work in `search_segmented`/`scan_segment_file_parallel` for scanner-bound queries (Q006, Q007, Q025, Q026, and all remaining scans).
4. **Posting hot-path proof reuse** — remove the per-dictionary validation `Mutex<HashMap<…>>` and the per-hit recency mutation in the decoded-posting cache (the storage plan's Change C; implement it here if not already done).
5. **FTS candidate quality and relation accounting** — make Q174's FTS estimate/runtime use the available bigram candidate, and correct the `relation_reverse_postings_visited` unit so relation work is measurable.

These are query-execution changes. They do not authorize changing query semantics, result ordering, storage layout, or the hybrid-storage repairs. Do not merge into `master`.

Non-goals:

- Do not redesign postings, dictionaries, bigram formats, sort columns, or relation storage beyond what each item requires.
- Do not change the meaning of FILTER/FTS, quantifiers, null/missing handling, or evidence outputs.
- Do not increase the verified-result cache or disable any cache to hide a regression.
- Do not move scanning work outside the query timer or into construction to make a benchmark pass.
- Do not implement page alignment, I/O-granularity changes, or a new cache.
- Do not implement a dense descriptor state table unless the posting-proof measurement gate in Section 8 requires it.
- Do not chase microsecond cases with high ratios (Q008, Q032, Q035, Q063). Report them but do not optimize them.

## 2. Base revision, branch and workflow

Start by recording the actual head of `integration/hybrid-index-storage`. The reviewed head is `471b79c3`; newer commits may exist when work starts. Inspect the full diff and preserve any sealing/constructed-reader work that has landed.

Read `AGENTS.md`, `docs/ai/README.md`, `docs/development/hybrid_index_storage.md`, `docs/search-core.md`, and the repair report at `bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md` before editing.

Workflow:

- This is a subtask of `hybrid-index-storage`; do not create a competing integration branch.
- Subtask branch: `task/hybrid-index-storage/query-scanner-plans`, based on the recorded integration commit.
- Subtask worktree: `/home/simon/source/objekte-task-hybrid-index-storage-query-scanner-plans`, a sibling of the original checkout.
- Keep `/home/simon/source/objekte` on `master`.
- The supervisor merges the subtask branch into `integration/hybrid-index-storage`, resolves conflicts, runs the combined validation and performance campaign, and reports.
- Push immediately after every commit. Do not force-push or rewrite published history.
- Run `cargo fmt --all`, `python3 scripts/format-rust-spacing.py`, `cargo test --workspace --locked`, and `cargo clippy --workspace --all-targets --locked` before each push.

Coordination:

- The other agent is working on sealing/validation/maintenance (`hybrid_segment_codec.rs`, `lifecycle.rs`, `posting_dictionary.rs`, `segment_format.rs`, `managed_store.rs`). Do not edit those files concurrently. Item 1 touches `posting.rs`, `index_catalog.rs`, `executor.rs`, and `schema_adapter.rs`; item 2 touches `reference.rs`; item 3 touches `run.rs`; item 5 touches `executor.rs`/`run.rs` and counters. Item 4 touches `posting_dictionary.rs`/`segment_format.rs` and must wait for the sealing series to merge.

## 3. Baseline evidence and current diagnosis

All numbers below are from `final-e15b` (220 cases × ASCII/Unicode × 30 repetitions per mode). No query campaign has been run since `471b79c3`, so these numbers are the current evidence, not an estimate of the present binary.

### Aggregate

| Mode | Median geomean | p95 geomean |
| --- | ---: | ---: |
| Memory | 1.075 | 1.238 |
| Fully resident | 1.091 | 1.116 |
| Warm on-demand | 1.092 | 1.105 |

### Dominant absolute regressions

| Case | Query | Baseline | Memory | Delta | Plan trace |
| --- | --- | ---: | ---: | ---: | --- |
| Q006 | unscoped FTS `"ü"` | 117.7 ms | 142.4 ms | +24.6 ms | restricted scanner, 148,486 candidates |
| Q007 | unscoped FTS `"a"` | 150.7 ms | 171.3 ms | +20.5 ms | restricted scanner, 148,486 candidates |
| Q187 | `contributors[*].type = contributors[*].type` | 100.8 ms | 116.6 ms | +15.8 ms | scanner, 133,948 candidates |
| Q202 | `contributors:array = true` | 92.4 ms | 107.7 ms | +15.3 ms | scanner, 133,948 candidates |
| Q204 | `contributors:? = false` | 83.8 ms | 97.5 ms | +13.7 ms | scanner, 133,948 candidates |
| Q203 | `contributors[*].type:null = true` | 75.2 ms | 85.4 ms | +10.2 ms | scanner, 133,948 candidates |
| Q025/Q026 | FTS `"Lied"` / `"Der"` | 58/61 ms | 70/72 ms | +11.9 ms | bigram candidate + scan |
| Q174 | FTS `"Liebe"` + `entry.year >= 1800` | 14.1 ms | 22.4 ms | +8.3 ms | scanner over 9,398 files |

Q167 (3.2 ms) and Q175 (2.8 ms, 95,153 relation memberships, 3,821 matches, zero source reads) are already index-proven and are not targets.

### Key observation

For every heavy case, the work counters are **identical** between baseline and candidate: same candidates, scanned records, parses, bytes read, matches. The traces select `scanner` over the full collection. The gap is CPU per unit of work in scanner-bound queries, not a different amount of work. The scan loop is nearly unchanged between `master` and the branch, so before changing code, verify that the aggregate delta is real and not measurement drift (Section 4).

### Why the scanner is selected

- `PostingIndex::structural(field)` creates `StructuralFacts` only when `field.indexed == true` (`posting.rs::structural`). `contributors` is a declared array field not present in `x-index-paths`, so it has no structural facts.
- `PostingAccess::assess` rejects structural state predicates with `query_path.has_explicit_array_step` (`((structural || Derived::Length) && term.query_path.has_explicit_array_step)`). That excludes `contributors[*].type:null = true` even though `contributors[*].type` is indexed and its `AnyNull` fact exists.
- `contributors:array = true`, `contributors:? = false`, and `contributors[*].type:null = true` therefore fall back to `AccessPathKind::Scanner`.
- `a = a` has `term.rhs.is_some()`, and `assess` rejects any term with an RHS, so it always scans.

## 4. Phase 0 — measurement sanity before optimization

Do this first. No query code change is authorized until the baseline is trustworthy.

1. **Interleaved A/B tail campaign.** Run the nine preselected tail cases (Q001, Q008, Q042, Q143, Q167, Q175, Q177, Q194, Q202) in an alternating order: baseline, candidate, candidate, baseline (ABBA), repeated to at least 30 samples per case/profile. Use the same fixture, schema, alphabet, backend, cache settings, and worker counts. Keep the existing `matched_tail.py` structure but alternate the two binaries instead of running two full campaigns. Archive under a new directory, e.g. `bench/results/hybrid-index-storage/query-optimization-20261115/tail-interleaved/`.
2. **CPU time and hardware counters.** For Q006, Q007, Q187, Q202, and Q175 record wall time, user+sys CPU time, and `perf stat` cycles, instructions, cache-misses, and page-faults for both binaries. Identical work counters plus proportional CPU-time/instruction deltas indicate a real code-cost difference; identical instructions with a wall-time gap indicates frequency/thermal or I/O effects.
3. **Profile.** Use `perf record -g` on Q006, Q187, and Q202 for baseline and candidate. Use the existing `detailed-profiling` feature only for phase attribution; run acceptance timings without it.
4. **Plan confirmation.** Export `EXPLAIN` traces for the five cases and record the chosen path. Confirm that the candidate is entering `scanner` for the expected reasons.

Gate:

- If the interleaved aggregate gap is below 1.05 for all modes, the parity gate is met; continue with items 1–5 as absolute improvements.
- If the gap persists, proceed with the same order but expect items 1–3 to close most of it.
- Do not report a campaign that mixes wall times from different sessions as a paired comparison.

## 5. Item 1 — structural predicates use structural postings

### 5.1 Required behavior

For a declared field, regardless of whether its values are value-indexed, these predicates must be answerable exactly from resident structural facts:

| Predicate | Structural source | Semantics |
| --- | --- | --- |
| `field:? = true/false` | `NonEmpty` | Any occurrence is nonempty; empty string/array/object are empty; zero/false are nonempty |
| `field:array` | `Array` | Any occurrence is an array |
| `field:object` | `Object` | Any occurrence is an object |
| `field:string` | `String` | Any occurrence is a string/logical ID |
| `field:number` | `Number` | Any occurrence is a number |
| `field:null` (multi) | `AnyNull` | Any occurrence is explicit null |
| `field:null` (single) | `nulls` | Explicit null; distinct from missing |
| `field:exists` | `presence` minus `empty_selections` | Path presence, including an empty selection |

These are the existing `StructuralKey` values. Do not add new semantics.

### 5.2 Where structural facts are created

Current code:

- `PostingCollection::new(schema)` includes a field when `indexed`, or when it is an array, or a non-multi string.
- `PostingIndex::structural(field)` sets `structural = field.indexed.then(StructuralFacts::default)`, sets `value_complete = field.indexed`, and leaves `lengths` empty.
- `add_values` always records presence/empty-selection/null/structural facts, and only fills `values`/`lengths` when completeness flags allow.

Required change:

1. Create a **structural-only** posting for every declared field that can be selected by the query binder, even if it is not value-indexed. This must not create a value dictionary.
   - Preferred shape: keep the existing `PostingIndex` but add an explicit `values_indexed: bool` (or `structural_only: bool`) persisted in the segment field binding. When false, `add_values` skips value and length insertion but still records `presence`, `empty_selections`, `nulls`, and `structural`.
   - Do not infer "structural-only" from `value_complete == false` plus empty maps; an indexed field can legitimately have no values, and inferred fields use completeness differently.
   - If a persisted flag is too invasive for the current format revision, derive it from the collection definition at decode time and document why it is stable. A silent mismatch between build and reopen is a correctness bug.
2. Include declared fields that are not value-indexed in `PostingCollection::new` when they can carry structural facts. At minimum: all array/object fields and all fields referenced by state-modifier queries. Measure the build/RSS cost of including all declared scalar fields too; choose the smaller set that makes behavior consistent.
3. Ensure inference/named/custom postings do not accidentally gain structural-only value dictionaries. Structural facts for inferred paths are a separate concern; do not change inference admission in this item.
4. Persist and restore the new flag. A reopened segment must not start filling values for a structural-only field on update. Add a mutation/compaction round-trip test.

### 5.3 Where structural predicates are routed

Current code:

- `PostingAccess::assess` in `index_catalog.rs` accepts structural terms (`term.derived.state_predicate().is_some()`), permits `LogicalType::Bool`, and calls `estimate_exact_with_visits_checked`.
- `PostingIndex::state_source` maps `Derived::{Null, IsNonEmpty, IsString, IsNumber, IsArray, IsObject}` to structural keys; `Exists` uses `presence`/`empty_selections`.
- `assess` rejects structural terms when `term.query_path.has_explicit_array_step`.

Required change:

1. Remove the blanket array-step rejection for **occurrence-existential** state predicates. Prove the equivalence:
   - Positive state predicates are existential over selected occurrences, so `AnyNull`/`Array`/`Object`/`String`/`Number`/`NonEmpty` are exact for `= true`.
   - `= false` is the complement over the record domain for the same existential aggregate, subject to the established missing/empty-selection rules. Use the existing `state_truth` logic, not a new negation.
   - Keep rejecting combinations that the binder already rejects (e.g., `:exists`/`:null` compared to another field; state predicate with a text binding or non-default quantifier).
   - Keep the guard for `Derived::Length` with an explicit array step unless separately proven; this item is only about state predicates.
2. Ensure the segmented path uses the same `assess` route as the direct path. `segment_expr_bounds` calls `field_postings(...).assess(...)` and `exact_set_restricted_checked`; verify no segment-specific guard silently returns `SegmentCandidateBounds::unknown` for derived terms.
3. Return `must` (exact) bounds when the posting coverage equals the segment domain and the term is exact. `may`-only candidates are not sufficient for Q202–Q204.
4. For fields that are missing structural facts because they are not declared (truly dynamic), keep the scanner. Do not infer structural facts at query time.

### 5.4 Expected effect

Q202, Q203, Q204 should move from ~100/85/97 ms to bitmap operations (sub-millisecond to low single-digit ms). Q187's rewrite (item 2) will then also be exact.

### 5.5 Risks and constraints

- Quantifier semantics are subtle. Every change needs differential tests against the existing evaluator over a fixture containing: missing path, explicit null, empty array, empty string, mixed arrays, nested arrays, multiple records per file, declared unindexed fields, indexed fields, inference fields, and named bindings.
- Memory growth: structural-only postings add presence/empty/null plus up to seven bitmaps per declared field. Measure before/after retained RSS and stored bytes. Do not add them to every field if the cost is material and the benefit is limited; prefer a documented set.
- Format compatibility: if a persisted flag is added, bump the relevant segment/field metadata version and configuration identity so old caches rebuild. The task's storage format is versioned; do not silently reinterpret old bytes.

## 6. Item 2 — self-comparison rewrite

### 6.1 Current behavior

`bind_term` builds an `rhs` `BoundFieldOperand` when the operand is a field. `eval_field_term` resolves both sides with `scoped_values` and compares paired or cross-product occurrences. `a = a` therefore scans and evaluates per record. Q187 costs ~100 ms for 133,948 records.

### 6.2 Required analysis

Prove the equivalence before implementing. For identical path, modifiers, transform, binding and scope:

- `a = a` with the default quantifier is the **existence** predicate for the path: true when the path is present, including an empty selection (universal vacuous truth); false when missing. This matches `Derived::Exists = true`, which uses `presence` minus `empty_selections`, not `:?`/`NonEmpty`.
- For `a != a`, the universal result over paired occurrences is false when occurrences exist and may be true for an empty present selection; do not rewrite unless the exact result is proven and separately tested.
- Explicit scopes must be identical on both sides; different scopes are rejected at binding today. Preserve that.
- If the RHS has a different transform, binding or derived state, do not rewrite.

### 6.3 Implementation

1. In binding, detect `lhs.path == rhs.path`, identical `query_path`, `derived`, `transform`, `operation`, `binding_name`, and `operator == Eq`, quantifier `Default`, and rewrite the term to the equivalent presence predicate (`Derived::Exists`, literal `true`). Keep the original term for EXPLAIN/diagnostics.
2. Alternatively, if a rewrite is judged too invasive for the term model, add a dedicated `PostingOperation::SelfCompare`/planner path that uses `presence`/`empty_selections`; do not evaluate per record.
3. Ensure the rewrite is reflected in EXPLAIN with a clear reason (e.g., `self-comparison reduced to path presence`).
4. Add a differential oracle: for every record in a fixture, compare the original evaluator and the rewritten path. Include missing, null, empty array, empty string, single/multi values, arrays of objects, duplicate scopes, and fields that are empty selections.

### 6.4 Gate

If the equivalence does not hold for a case, leave Q187 on the scanner and report why. Do not ship an approximate rewrite.

## 7. Item 3 — scanner allocation and evidence reduction

Target `run.rs` scan paths:

- `search_segmented_impl` (~3784)
- `scan_segment_file_parallel` (~3399)
- `verify_local_segment_record` (~3236)
- `DirectSourcePlan` (~656)
- candidate grouping around the `files: BTreeMap<u32, SmallVec<…>>` block (~4242)

### 7.1 Evidence only for page candidates

Currently `verify_local_segment_record` creates a `HashSet<String>` per record and populates evidence during matching, even for records that cannot enter the top page. With `cap = 50` and 65k–148k matches (Q006/Q007), this allocates and hashes for every match.

Required:

1. Evaluate membership without evidence for all records (`retain_evidence = false`), and collect evidence only for records that are offered to a winner/page. Re-evaluate only those records (or cache the minimal evidence) with evidence enabled.
2. For unscoped FTS (`fts.fields.is_empty()`, no filter), evidence is a fixed or empty set. Avoid per-record `HashSet` allocation entirely; emit the static evidence for winners.
3. Preserve `matched_fields` exactly: compare evidence output for every case in the campaign and for targeted fixtures. Evidence omission must never change the returned evidence list, only when it is computed.

### 7.2 Direct/borrowed scanner coverage

Confirm which queries use `scan_borrowed_segments_worker` and which still build a full `Value`. Extend the direct path only where semantics are already implemented:

- unscoped FTS must remain literal scalar-segment matching (property names, punctuation excluded; numbers by source spelling; booleans/null as text; malformed fallback raw text);
- a single `Contains` filter term is already covered;
- do not add structural predicates to the borrowed path in this item; item 1 should remove their scan entirely.

If a query cannot use the direct path, document the reason in EXPLAIN or a trace (not a new public field unless already available).

### 7.3 Candidate grouping without a tree

`files: BTreeMap<u32, Vec<...>>` is built per segment from a slot-ordered bitmap. If segment record slots are file-ordered (verify the construction invariant), replace the tree with:

- one pass over `candidates` producing `Vec<(file_id, slot, range)>`, then `sort_unstable_by_key(file_id)` and chunk runs; or
- an iteration over contiguous file runs when the invariant is proven.

Keep the `SmallVec<[…; 1]>` inline behavior for one-record files. Freeze the invariant with a test that asserts file IDs are nondecreasing in slot order for a multi-record fixture; if the invariant is not guaranteed, use the sorted vector.

### 7.4 Other per-record costs

Audit and remove, with measurement:

- per-record `RecordContext` construction where fields are constant;
- per-record `String`/`PathBuf` joins (precompute per file);
- repeated `HashSet`/`Vec` allocations in `eval_bound_fts`/`eval_filter_truth` where a reusable scratch buffer is possible;
- `String::from_utf8_lossy` repeated for the same bytes in fallback paths.

Do not change text matching semantics.

### 7.5 Expected effect

Q006/Q007 and Q025/Q026 lose allocation/hash overhead proportional to matches; Q187 may be removed by item 2. Target: at least 10% improvement on the scanner cases and no regression on the index-proven cases.

## 8. Item 4 — posting hot path without a per-dictionary validation map

Implement the storage plan's Change C if it is not already merged:

1. Remove `SealedPostingDictionary.validated: Mutex<HashMap<PostingDescriptor, Arc<OnceLock<Result<(), String>>>>>` and `with_validation`.
2. A cache hit is proof; a large miss validates once and enters `DecodedPostingCache`; a small (<64-byte) miss validates on access.
3. Keep errors deterministic and never convert a corrupt posting to empty or absent.
4. Convert `DecodedPostingCache` recency from per-hit `BTreeMap` remove/insert under `Mutex` to a clock or O(1) scheme. Optional: `RwLock` for hits. Do not increase the cache.
5. Verify dictionaries sharing a segment use the same eligible domain or key the cache by `(descriptor, domain fingerprint)`.

Measurement gate for a dense descriptor table: only after the above, if sub-64-byte validation dominates, consider a writer-assigned descriptor ordinal plus `Box<[AtomicU8]>`; include its memory in the budget. Do not implement it speculatively.

## 9. Item 5 — FTS candidates and relation accounting

### 9.1 Q174 FTS candidate

The Q174 plan reports `fts_strategy: "compatible bigram candidate or restricted scanner"` with `fts_estimated_candidates = 133948` even though the actual scan touches 9,398 files. Either the estimate is conservative or the runtime candidate is not being used.

Required:

1. Trace `prepare_fts_accesses` / `segment_fts_candidates` for `fts = "Liebe"` in `contents`. Confirm whether a `BigramRequest` is produced and whether `candidates_checked` returns a restriction.
2. If the candidate exists but the estimate ignores it, fix the estimate to reflect the intersection; do not change runtime semantics.
3. If the candidate is discarded because of a field/binding condition, document the condition and, if safe, allow the raw bigram candidate for unscoped FTS over a collection whose raw text covers identity fields. Preserve exact verification.
4. Add a plan/counter check that `fts_estimated_candidates` is not the full collection when a non-empty gram request is available.

### 9.2 Relation counter units

`relation_reverse_postings_visited` currently adds `slots.len()` on the segmented path and uses a different unit on the direct path. Q175 reports 95,153 while only 3,821 targets match. Define comparable posting-list/member counters:

- `relation_reverse_postings_visited`: number of posting **lists** (dictionary entries/keys) visited;
- `relation_reverse_members_visited`: number of posting **members** (slot cardinalities) visited;
- keep the old field for compatibility but document and populate it consistently, or rename with a versioned workload field.

Do not infer a 25-fold work increase until the units match. This is measurement correctness, not an optimization.

## 10. Correctness and regression tests

Add focused tests; do not duplicate existing coverage.

### Structural predicates

1. For a fixture with missing, null, empty string, empty array, nonempty array, mixed array, nested array, object, and multiple records per file, assert `:?`, `:array`, `:object`, `:string`, `:number`, `:null`, `:exists` equal the scanner/evaluator result for `= true` and `= false`.
2. Cover declared unindexed fields, indexed fields, explicit `[*]` scopes, default and `?` quantifiers, and `each`.
3. Assert exact `must` bounds for Q202–Q204-shaped terms in `EXPLAIN`: `kind: posting`, `exact: true`, `coverage: full`.
4. Round-trip through memory, fully resident, on-demand, flush/reopen, update/delete/compaction, inference enabled/disabled, and named bindings. A reopened segment must preserve `structural_only`.

### Self-comparison

5. Differential oracle for `a = a` and `a != a` over the fixture, including empty selections; only the proven rewrite is enabled.
6. `EXPLAIN` records the reduction and its reason.

### Scanner and evidence

7. For every evidence-producing case, compare `matched_fields` before/after. Missing evidence can be ordered differently? Preserve the documented set/order behavior; if ordering is sorted, keep it sorted.
8. Candidate grouping invariant: file IDs are nondecreasing in slot order for multi-record files, or the sorted-vector fallback is exercised.
9. Cancellation, execution context limits, and error paths still return the same error categories.

### Posting hot path

10. Corrupt posting payload with an external store: every scalar/range/structural/length/bigram/FTS/relation path errors, never empty.
11. Two-thread first access to the same uncached posting; eviction and revalidation; small-posting repeated access.
12. Different eligible domains in one segment either share a proven domain or use separate cache entries.

### FTS / relation

13. Q174-shaped query returns identical hits and evidence; FTS candidate estimate is not full when grams exist.
14. Relation counters are internally consistent and reproducible; no result changes.

Run the workspace test suite, Clippy, formatting, CLI process tests, and native desktop checks as documented.

## 11. Measurement plan

Use the established archive conventions and a new result directory, e.g. `bench/results/hybrid-index-storage/query-optimization-20261115/`.

### Commands

- Tail, interleaved: adapt `bench/results/hybrid-index-storage/repair-20261113/scripts/matched_tail.py` to alternate baseline/candidate binaries per round (ABBA) for the nine preselected cases, 30 repetitions, ASCII and Unicode, verified-result cache disabled, prepared cache 16 MiB.
- Full campaign: `bench_musenalm --root examples/musenalm --extended --incremental --profiles ascii-off,unicode-off --repetitions 5 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32`, plus a 30-repetition tail for the selected cases.
- Memory mode: baseline binary vs candidate memory policy. Stored modes: candidate fully resident and warm on-demand; compare open and query results to the same baseline binary as the task requires.
- Construction and open: retain the existing seven paired trials to detect memory/build regressions from structural postings.
- Memory: retained/peak RSS after build and after open; stored bytes; structural posting estimate.

### Acceptance

- All result digests and counts identical in memory, fully resident, and on-demand.
- Aggregate median geomean ≤ 1.05 in each mode; p95 geomean ≤ 1.10; report absolute deltas and ratios.
- Q202/Q203/Q204: target < 5 ms each (from ~75–108 ms).
- Q187: target < 10 ms or documented as unresolved with the semantic reason.
- Q006/Q007, Q025/Q026: at least 10% improvement or a documented explanation; no regression.
- Q167/Q175/Q177: no regression; relation counters comparable.
- Construction: no more than 2% slower; retained/peak RSS no more than 5% higher; stored bytes within 5%. Investigate any larger change.
- Cold-index and constrained-memory behavior are not established by warm-cache trials; state that limitation.

## 12. Commit sequence

Each commit is self-contained, tested, pushed, and independently measurable.

1. **Phase 0 evidence** — interleaved tail harness, CPU/perf measurements, baseline archive; no behavior change.
2. **Structural-only postings** — create and persist structural facts for declared non-indexed fields, with build/restore/update tests and memory measurement.
3. **Structural routing** — allow occurrence-existential state predicates with explicit array steps; wire exact bounds; Q202–Q204 plans and tests. Include `EXPLAIN` evidence.
4. **Self-comparison** — proven `a = a` rewrite with differential tests; Q187.
5. **Scanner evidence/grouping** — lazy evidence, static unscoped evidence, candidate grouping; Q006/Q007/Q025/Q026 tests and measurement.
6. **Posting hot path** — remove the validation map and recency mutation; focused measurements. (May be merged earlier if the sealing series lands first.)
7. **FTS candidate/counters** — Q174 candidate estimate; relation counter units; docs.
8. **Documentation and final campaign** — update `docs/search-core.md`/`docs/development/hybrid_index_storage.md` for structural fact coverage, planner behavior, counters, and any persisted flag; run the full campaign; write the report.

If a step fails its correctness gate, keep it out and report the limitation; do not substitute a different approximation.

## 13. Documentation updates

- `docs/search-core.md`: structural-only posting coverage, exact state predicates, occurrence-existential semantics with array steps, self-comparison reduction, scanner evidence behavior, candidate grouping invariant.
- `docs/development/hybrid_index_storage.md`: persisted `structural_only` flag (if added), format/version implications, cache/validation behavior, structural posting memory accounting.
- `docs/cli/obj.md` / TUI/desktop docs only if EXPLAIN/status fields change.
- Workload/plan trace docs: relation counter units.

## 14. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Structural state semantics differ for quantifiers/scopes | Differential oracle against the existing evaluator; reject any case without proof |
| Structural-only postings increase memory or build time | Measure per-field; include only the documented set; report retained/stored bytes; no more than the agreed budget |
| Persisted flag breaks reopen/update | Version bump + configuration identity change; round-trip tests for memory/resident/on-demand and compaction |
| Self-comparison rewrite is unsound for empty selections | Only rewrite the proven `Eq` case to `Derived::Exists`; property tests; keep `Ne` on the evaluator |
| Lazy evidence changes output | Compare full evidence sets for all campaign cases and fixtures; only compute later, never omit |
| Candidate grouping assumes slot order | Prove with a test or use the sorted-vector fallback |
| Posting cache proof removal hides corruption | Keep checked materialization for uncached external postings; corruption tests; errors never become absences |
| Measurement drift | Interleaved ABBA, CPU time, perf counters, one stats definition, fresh processes, record provenance |
| Draft overlaps the sealing agent's files | Work only in the assigned files; serialize `posting_dictionary.rs`/`segment_format.rs` edits with the supervisor |

## 15. Deliverables and completion

1. Pushed commits on `task/hybrid-index-storage/query-scanner-plans`, merged by the supervisor into `integration/hybrid-index-storage`.
2. `bench/results/hybrid-index-storage/query-optimization-20261115/REPORT.md` with:
   - revisions, binary hashes, commands, fixture/schema/workload fingerprints, cache settings;
   - interleaved tail and full-campaign results with raw samples;
   - per-case before/after for Q006/Q007/Q174/Q187/Q202/Q203/Q204/Q025/Q026/Q167/Q175/Q177;
   - structural posting memory and stored-byte accounting;
   - evidence parity, result digest parity, and correctness/failure-test results;
   - disposition for items 1–5 (implemented, verified, incomplete) and remaining limitations.
3. Updated docs listed in Section 13.
4. Keep the authoritative task document in `docs/ai/tasks/` and do not move it to `done/` unless the supervisor confirms the whole `hybrid-index-storage` scope is integrated and validated. Do not merge `master`.

Definition of done for this subproject:

- Q202/Q203/Q204 no longer scan; their plans are exact structural posting lookups with identical results.
- Q187 is index-proven or explicitly documented as semantically unsafe to rewrite.
- Scanner-bound cases show reduced allocations and no result/evidence changes.
- Posting access has no per-dictionary validation mutex or per-hit recency mutation.
- Q174's FTS candidate is understood and either improved or documented; relation counters have consistent units.
- Full correctness suite passes; interleaved and full campaigns meet the stated gates, with remaining regressions reported honestly.


## Implementation report

- **Task identifier and closure date:** `hybrid-index-storage` query optimization subproject, 2026-11-14.
- **Base and integration:** Actual base `8ca2851ab8` on `integration/hybrid-index-storage`; subtask branch `task/hybrid-index-storage/query-scanner-plans` and sibling worktree `/home/simon/source/objekte-task-hybrid-index-storage-query-scanner-plans`. The subtask was fast-forwarded into `integration/hybrid-index-storage`, whose retained worktree is `/home/simon/source/objekte-integration-hybrid-index-storage`. `master` was not changed.
- **Implemented:** Structural facts for all declared fields with persisted `values_indexed`; exact routing of proven state predicates across explicit array steps; exact same-field equality from raw path presence; borrowed-source scanner grouping and deferred evidence; atomic clock bits for decoded-posting read hits; improved segmented FTS candidate estimates and separate relation-list/member counters. The ordered subtask commits are `32b00f5137`, `5020aa328d`, `0a80e830a4`, `a093684647`, `3b959f1055`, `4a1c2a8883`, `aaf732dd93`, and evidence commit `293a10498c`.
- **Validation:** The combined integration revision `74cab22411` passed `cargo test --workspace --locked --quiet` and `cargo clippy --workspace --all-targets --locked --quiet` with Clippy warnings. Differential, stored-round-trip, corruption, and concurrency tests passed. `cargo fmt --all -- --check` and the repository spacing check report pre-existing broad formatting drift; no global formatting change was included.
- **Benchmarks:** `bench_musenalm --root examples/musenalm --extended --incremental --profiles ascii-off,unicode-off --repetitions 5 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32` ran 220 cases in both profiles. A 30-repetition focused A/B/B/A tail, seven alternating construction pairs, and seven source-free open pairs were archived with exact commands, hashes, fingerprints, result digests, plan traces and raw samples in the [final campaign report](../../../bench/results/hybrid-index-storage/query-optimization-20261115/REPORT.md). All 440 case/profile digests matched; memory median geomean ratios were 0.789 ASCII and 0.747 Unicode. Q187 and Q202–Q204 became exact postings with sub-millisecond medians.
- **User-approved deviation and remaining work:** The user instructed us to ignore further performance evidence when closing these documents. Construction's measured 1.041× ratio, 4–6% source-free open regression, and scanner cases below the 10% target remain recorded rather than described as passing. The five requested code changes are implemented; no query-correctness gap is currently known. Parent-scope gaps were transferred to the issues linked from the main and repair completion reports. The task branch is pushed; its clean worktree was removed after integration. The integration worktree remains for review.
