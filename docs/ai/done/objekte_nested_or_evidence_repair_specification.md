# Objekte repair specification · Prevent duplicate posting lookups in nested OR queries

**Merge condition.** With matched-field evidence enabled, an ordered query that has to fall back must apply the indexed predicate `a = true` exactly once per segment on the same input. Both `IndexView::search` and `IndexView::search_stream` must return the reference count, ordered rows and matched fields at all four page bounds below.

Continue the existing `integration/query-execution-path-repair` task in `/home/simon/source/objekte-integration-query-execution-path-repair`. The reviewed remote tip is `5d51345d9801fc93ef6abb934ba55ed3a26a4ecb`. This addendum covers the ordered OR evidence probe and its regression tests. Follow `docs/ai/README.md` and leave the merge to the owner.

## 1. Reproduce the duplicate

Use `indexed_or_file_layout(65)` in `crates/core/src/query_execution/runner/tests.rs`. It builds one segment with 260 active slots in 65 JSONL files, four query workers and the verified-result cache disabled. Use the stored ascending numeric sort on `sort`, locator output, count enabled, evidence enabled, offset 1 and limit 50. Leave FTS unset. [Fixture](https://github.com/Simon-Martens/objekte/blob/5d51345d9801fc93ef6abb934ba55ed3a26a4ecb/crates/core/src/query_execution/runner/tests.rs#L7163-L7254)

```text
a = true || (b = true && (sort = 0 || note = 'needle'))
```

The ordered probe checks the first child's evidence fields and applies the `a` posting. The inner OR can contribute either `sort` or `note`, so its enclosing AND has no single set of evidence fields known in advance. The probe returns `Ok(None)` when it reaches this second child. The caller has nothing to insert into its request-local cache. General execution then evaluates the top-level OR on the same active slots and applies `a` again. The result rows can still be correct, so a result-only test misses the defect. [Evidence classification and probe](https://github.com/Simon-Martens/objekte/blob/5d51345d9801fc93ef6abb934ba55ed3a26a4ecb/crates/core/src/query_execution/runner/mod.rs#L1334-L1406) · [Caller and cache insertion](https://github.com/Simon-Martens/objekte/blob/5d51345d9801fc93ef6abb934ba55ed3a26a4ecb/crates/core/src/query_execution/runner/mod.rs#L3678-L3715)

Add the nested regression first. Obtain `search_reference` results before starting predicate observations. For each public call independently, start observations, execute the whole call and take its observations. For streaming, collect every ordered batch and require `StreamOutcome::Complete`. On the reviewed tip the source predicts two successful applications of `a` on slots 0–259. Capture the actual baseline separately for each API, then run the identical test after the fix.

## 2. Check every immediate OR child before applying any restriction

In `exact_or_branch_evidence_fields` in `crates/core/src/query_execution/runner/mod.rs`, separate eligibility from index work. The existing recursive `segment_static_evidence_fields` already answers the eligibility question for a child.

1. Immediately after matching `BoundExpr::Or(parts)`, obtain the evidence-field list for each immediate child in expression order. Collect into `Option<Vec<_>>`. If any child returns `None`, return `Ok(None)` before taking the active bitmap or calling a restriction provider.
2. Once the list is complete, iterate over `parts.iter().zip(child_fields)`. Move each saved field list into the existing branch-evidence calculation. This visits the classifier once per immediate child in the helper and requires no copy of the expression or posting bitmaps.
3. Retain the existing behavior for children with known field names whose membership still needs verification. Finish combining every child's possible and proven slots, return the aggregate, and let the caller cache it before declining the ordered route. The flat residual query below depends on this distinction.
4. Apply eligibility per child. Children with different known field lists, such as `a` and `b`, are eligible together. They retain their separate field lists so a fully indexed OR can still return per-row evidence without reading source files. Use the existing fallback and request-local restriction cache.

## 3. Acceptance tests

Exercise the nested expression above and the existing flat control `a = true || note = 'needle'` with **evidence enabled**, on **both public APIs**, at every row of this table. Both expressions match 257 records on this fixture. IDs 1, 5 and 9 are the only exclusions. Preserve the existing evidence-disabled tests. [Current page-bound table](https://github.com/Simon-Martens/objekte/blob/5d51345d9801fc93ef6abb934ba55ed3a26a4ecb/crates/core/src/query_execution/runner/tests.rs#L7574-L7645)

| Offset | Limit | Offset + limit | Returned rows | First ID | Last ID |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 50 | 51 | 50 | 2 | 53 |
| 14 | 50 | 64 | 50 | 17 | 66 |
| 15 | 50 | 65 | 50 | 18 | 67 |
| 0 | 260 | 260 | 257 | 0 | 259 |

- **Physical work.** For each nested query call, select observations for field `a`, operator `Eq` and literal `Bool(true)`. Assert exactly one observation overall and verify its segment and complete input slot set. The existing `assert_posted_a_once` also asserts that `note` receives all 260 slots. Use that helper for the flat control. Give the nested case its own assertion for `a`, because its inner AND may legitimately narrow the input to `note`. Count provider applications separately from source reads.
- **Results and evidence.** Compare the complete hit vector and count against the reference for each call. At offset 1, nested ID 2 must report the fields `a`, `b`, `note`, and ID 13 must report `b`, `note`. For the flat control those same IDs report `a`, `note` and `note`, respectively. For the full nested page, ID 0 reports only `a` because its `b` branch is false.
- **Existing controls.** Keep the fully indexed `a = true || b = true` evidence case on the ordered route with zero files opened, bytes read and records parsed. Keep the broad and selective FTS checks in the existing test. Reuse its fixture, reference evaluator and predicate observation hooks. [Exact OR control](https://github.com/Simon-Martens/objekte/blob/5d51345d9801fc93ef6abb934ba55ed3a26a4ecb/crates/core/src/query_execution/runner/tests.rs#L7258-L7325)

## 4. Verification and handoff

Run the focused regression first. The command below selects the existing test. If you add a separately named nested test, run it explicitly as well and verify that the test filter selected it.

```sh
cargo test --locked -p objekte-search-core --all-features ordered_exact_multi_file_or_does_not_reapply_restrictions_in_fallback
cargo test --locked -p objekte-search-core --all-features
cargo test --locked --workspace
git diff --check 5d51345d9801fc93ef6abb934ba55ed3a26a4ecb HEAD
```

Append the observed before-and-after posting counts for each API, the page-matrix results, suite totals and implementation SHA to the completed task document under `docs/ai/done/`. Push the implementation and report to the existing integration branch and identify both revisions. Operation counts and reference-hit equality are the acceptance evidence for this repair.

## Implementation report

- Task identifier: `nested-or-evidence-repair`
- Completed: 20 November 2026
- Starting revision: `5d51345d9801fc93ef6abb934ba55ed3a26a4ecb`
- Final code revision: `5ddfb0996553b212dd7f9e9afe988cabb1d84e8f`
- Integration branch: `integration/query-execution-path-repair`
- Integration worktree: `/home/simon/source/objekte-integration-query-execution-path-repair`

### Implemented repair

`5ddfb099` separates evidence eligibility from restriction work for top-level OR children. It first classifies every immediate child with `segment_static_evidence_fields` in expression order. If any child is ineligible, the helper returns `Ok(None)` before taking the active bitmap or invoking a restriction provider. Eligible children then use the preclassified field lists for the existing possible/proven combination and exact-evidence calculation.

The nested child `b = true && (sort = 0 || note = 'needle')` is therefore rejected before the top-level `a` posting runs. General execution owns the query and evaluates it once. Flat children such as `a` and `note` remain eligible; they still combine aggregate bounds and cache them for fallback evidence evaluation.

### Posting observations

The nested expression was run on one segment with 260 active slots:

```text
a = true || (b = true && (sort = 0 || note = 'needle'))
```

| Entry point | Baseline `a` applications | Repaired `a` applications |
| --- | ---: | ---: |
| `search_stream` | 2 | 1 |
| `search` | 2 | 1 |

The repaired observation uses segment `1`, field `a`, operator `Eq`, literal `Bool(true)` and the complete input slot set `[0..259]`. General execution performs the source verification needed to produce complete fields. The flat control `a = true || note = 'needle'` continues to apply `a` and `note` once each on the full input.

### Page and evidence matrix

Both expressions were run independently through both public APIs at every required page bound. Every call matched `search_reference` in count and the complete hit vector.

| Expression | Offset / limit / bound | Returned | First ID | Last ID |
| --- | --- | ---: | --- | --- |
| nested | 1 / 50 / 51 | 50 | 2 | 53 |
| nested | 14 / 50 / 64 | 50 | 17 | 66 |
| nested | 15 / 50 / 65 | 50 | 18 | 67 |
| nested | 0 / 260 / 260 | 257 | 0 | 259 |
| flat | 1 / 50 / 51 | 50 | 2 | 53 |
| flat | 14 / 50 / 64 | 50 | 17 | 66 |
| flat | 15 / 50 / 65 | 50 | 18 | 67 |
| flat | 0 / 260 / 260 | 257 | 0 | 259 |

At offset 1, nested ID `2` reports `["a","b","note"]` and ID `13` reports `["b","note"]`; flat ID `2` reports `["a","note"]` and ID `13` reports `["note"]`. On the nested full page, ID `0` reports only `["a"]`. IDs `1`, `5` and `9` are absent from all appropriate full result sets.

Fallback source verification opened more than 64 files and scanned at least 257 records in every evidence-bearing residual call. Provider application counts were observed separately from source reads.

### Existing controls

- Fully indexed `a = true || b = true` evidence continues on the ordered route with zero files opened, zero source bytes and zero source parses.
- Broad `needle` and selective `extra` FTS controls retain full result parity and structured-posting reuse.
- Evidence-off page bounds 51, 64, 65 and 260 still apply `a` and `note` once.
- The selective ordered stream still emits ID `12` after one candidate and one source verification.
- Cursor allocation and nested evidence/cache regressions remain green.

### Validation

- `cargo test --locked -p objekte-search-core --all-features`: 633 passed, 16 ignored, 0 failed; all integration and doc tests passed.
- `cargo test --locked --workspace`: passed for core, CLI and desktop; core ran 632 passed and 16 ignored under the workspace feature set.
- Focused nested/flat evidence, FTS, page-bound, selective-route, cursor-allocation and exact-OR tests passed.
- Focused `rustfmt --edition 2024 --check` passed for both task-owned Rust files.
- `cargo fmt --all -- --check`: only the inherited extra blank lines in `crates/core/src/model/json.rs` remain.
- `git diff --check`: passed.

The full timing suite was omitted because this repair changes only evidence-probe scheduling and the specification requires operation counts and reference equality rather than timing.

### Delivery and cleanup

The implementation is pushed on the integration branch. Task memory was removed after transferring its findings here. The original checkout's untracked task source was removed only after the committed 54-line specification was verified byte-for-byte. The integration branch and worktree remain for owner review and merge.
