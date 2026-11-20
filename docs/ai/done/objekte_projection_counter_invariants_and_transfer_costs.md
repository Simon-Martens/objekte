# Objekte — repair projected inference counts and remove first-batch accounting overhead

## Assignment

Continue in the worktree for `repair/path-posting-query-evaluation`. The reviewed head is `514d46bdfa8af61d94549e202b2f71e16d4085ee`. The measured implementation is `b80e5fad87`. The deliverable is a tested repair on that branch and a short implementation report. The user performs the final merge.

Implement the work in this order:

1. Add the three failing survivor cases in §1. Record the failing assertions on the reviewed revision.
2. Correct the common projection finalizer using §§2–4. Pass the fresh-build comparisons, partition tests, production validator and consolidation tests in §5.
3. Move transfer counting inside the profiling branch and remove one overwritten clone using §6.
4. Run §7 and report the actual results.

Keep the working fixes for numeric `integer_only`, terminal scalars reached through arrays, small-array insertion and first-batch ownership transfer. The repair can be implemented using existing counts, summaries and posted bitmaps. Changing query semantics, storing another permanent structural bitmap, reading source files during projection, or redesigning index construction would exceed this assignment.

## 1. Write the reproductions first

Use one logical JSON record per file, an empty schema, and a single source segment containing all fixture records. Give files stable, unique names. Derive or verify the slot-to-file mapping before retiring slots. The expected values below refer to the state **after** retirement.

| Case | Input in file order | Mode and survivors | Exact reference facts and failing behavior |
| --- | --- | --- | --- |
| A. Retired empty arrays | A0 `{"x":[]}`, A1 `{"x":[]}`, A2 `{"x":[7]}` | AutoIndex. Keep A2. | For `x`: eligible 1, present 1, missing 0, array records 1, empty-array records 0, empty-selection records 0. The source report had two empty arrays. Current projection retains 2 and fails validation against the new array-record count of 1. |
| B. Mixed terminal types | B0 `{"x":[]}`, B1 `{"x":{}}`, B2 `{"x":null}`, B3 `{"other":true}` | AutoIndex. Keep B0, B1, B2. | The length capability admits `x`. Fresh record counts are present 3, missing 0, scalar 1, object 1, array 1, empty array 1, empty selection 1, null occurrence 1, singleton null 1. The scalar is the explicit null. The object and null supply no ordinary scalar dictionary kind. Current projection can therefore report zero for both shape counts. |
| C. Report without an index | C0 `{"label":"Ada"}`, C1 `{"other":true}` | ReportOnly. Keep C0. | For `label`: eligible 1, present 1, missing 0, scalar 1, nonblank records 1. No posting is expected. Current projection sets presence to zero and retains one nonblank record, which the validator rejects. The corrected report stays incomplete. |

Assert these fixture preconditions on the original segment: A has a complete lengths posting whose zero key contains two slots; B has a lengths posting and independently records one object and one scalar in its report; C has an observed report row and no corresponding posting. A zero-row or incorrectly admitted fixture must fail the test setup. [Builder accounting](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/construction/inference.rs#L1514) · [Existing compaction tests](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/index/segment/compaction.test.rs#L278)

## 2. Work in the common projection finalizer

Edit `crates/core/src/index/segment/projection/inference.rs::finish_projected_inference`. Both `project_inference` and `project_inference_for_partitions` already call it after remapping the postings. Put the count rules here once. Keep the existing remapping of bitmaps and removal of empty short-string coverage entries. [Both entry points](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/index/segment/projection/inference.rs#L14)

The field report starts as a copy of the source report. In the formulas below, `old.foo` means the value **on entry** to this finalizer. Save the required numeric inputs in local variables before overwriting them. Avoid cloning the whole field report or any bitmap to preserve a few integers.

| Symbol | Meaning |
| --- | --- |
| `E` | Number of retained logical records, from the function’s `eligible` argument. Physical records may also include fallback records. |
| `T` | Number of surviving slots in the path’s terminal-presence bitmap, `index.presence.len()`, when a posting exists. |
| `N` | Cardinality of the projected `ScalarKey::Null` posting, defaulting to zero when the path has complete terminal evidence and the key is absent. Array-element nulls are a different observation. |
| `Z` | Cardinality of the projected length-zero posting, defaulting to zero **when length coverage is complete**. When coverage is absent, this quantity is unknown. |
| `P`, `A`, `EA` | The output record counts for legacy presence, terminal arrays and terminal empty arrays, respectively. A value can be exact or a conservative upper bound as specified below. |

Legacy presence includes a successful empty traversal. For example, `items=[]` can count as present in the inference report for `items.name` while selecting zero terminal names. Likewise, a field selecting one array with two strings has one terminal array, zero terminal scalars, and can have two scalar **value occurrences** from the elements. Scalar, array and object record counts can overlap across a multivalued path. Never obtain one by subtracting the other two.

## 3. Assign the fields in dependency order

The following inherited bounds apply to fully observed source domains and to already-projected reports carrying valid bounds. A complete collection observation can contain an incomplete **field** report. Keep `state.report.complete` (observation coverage) separate from `field.complete` (accuracy of the projected field facts). For an observation that actually stopped early, its partial observed count need not bound the real population. Preserve its incomplete status. For record-count fields that require an upper bound in that case, use the known surviving parent population as the ceiling unless there is stronger posting evidence. The regression oracle uses complete source observations in both AutoIndex and ReportOnly modes.

**3.1 Domain and presence.** Set `eligible_records = E` and `unavailable_records = new_count - E` with the existing checked subtraction. With a posting and a terminal summary that rules out an empty traversal, set `P = T`. With a possible empty traversal, no posting, or unknown traversal evidence, use `P = min(old.present_records, E)` for the inherited-bound case. A known `T` is a lower bound on `P`. Set `missing_records = E - P` exactly once, after presence has been finalized. When `P` is an upper bound, this yields a **lower** bound on missing records.

**3.2 Keep the proved scalar case.** Preserve the existing exact branch for a complete, single-valued, non-string scalar dictionary with no selectable container and no empty-ancestor traversal. Use terminal evidence to establish that shape. In this branch `P=T`, `scalar_records=T`, `array_records=object_records=empty_array_records=empty_selection_records=0`, and both null counters equal `N`. For the supported scalar case, `value_occurrences=T-N`. Preserve the existing recomputation of numeric `integer_only` from surviving keys and the exact numeric regression: keeping only integer 7 after retiring 1.5 gives `Some(true)`. If no numeric value survives, use `None`. An exact calculation for one counter does not promote an already incomplete field to complete.

**3.3 Use inherited record bounds for the general case.** Explicitly mark this branch `field.complete = false`. Apply the following table. The same parent bounds apply to paths without postings, including ReportOnly and otherwise unadmitted fields. Leave their index status on the existing incomplete projection path and advertise zero index bytes and zero short-string posting coverage.

| Output | Assignment for a fully observed parent |
| --- | --- |
| `scalar_records` | `min(old.scalar_records, P)`. Source terminal nulls already count here. |
| `object_records` | `min(old.object_records, P)`. The builder tracks terminal objects directly. |
| `array_records = A` | `min(old.array_records, P)`. A complete zero-length posting provides a proven subset of this population. |
| `empty_array_records = EA` | If terminal length coverage is complete, assign `Z`. Otherwise assign `min(old.empty_array_records, A)`. Assign this field on every branch, including the exact-scalar and no-posting branches. |
| `empty_selection_records` | If terminal evidence rules out empty-ancestor traversal, use `EA` (exact whenever `EA` is exact). Otherwise use `min(old.empty_selection_records, P)`. This bounds the union of terminal empty arrays and successful empty traversals, including overlap. |
| `null_occurrence_records` | If complete terminal evidence rules out a terminal array, assign `N`. Otherwise use `min(old.null_occurrence_records, P)`. The latter includes possible null array elements. |
| `exact_null_records` | With complete terminal evidence and at most one selected terminal per record, assign `N`. With multiple terminals possible, use `min(old.exact_null_records, N, null_occurrence_records)` if `N` is known. Without terminal evidence use `min(old.exact_null_records, null_occurrence_records)`. |
| `nonblank_records` | `min(old.nonblank_records, P)`. A nonblank array element can contribute to this count. |
| `value_occurrences`, `nonblank_occurrences`, `short_string_observed_occurrences` | Set to zero when `P=0`. Otherwise retain the source occurrence upper bounds in the incomplete branch. A surviving record may contain many values. A clamp to `E` would undercount. |
| `integer_only` | Use `None` when the general branch cannot prove the property from the surviving population. |
| Short-string accelerated coverage | Compute `short_string_covered_records` from the retained coverage bitmap, otherwise zero. Set `short_string_unresolved_records = E - short_string_covered_records`; preserve existing distinct-value and coverage validation. A reported nonblank record in ReportOnly mode does not create posting coverage. |

Remove the general branch’s `scalar_kind`, `array_kind` and `object_kind` tests based on `observed_kinds`. Leave observed dictionary kinds intact for their actual uses. Remove the blanket assignment of zero presence or zero shapes for a field without a posting. Use terminal evidence only where the table grants it. For example, on input B keeping only the object, it is acceptable for an incomplete field to retain scalar and array upper bounds of one; its **exact** length-zero count must be zero. The object upper bound must include one.

**3.4 Empty domain and report bookkeeping.** If `E=0`, every record count, occurrence count and missing/coverage count in the table is zero. Keep the actual physical unavailable-record count and ordinary index validation. Distinguish this from a nonempty domain in which a specific path has no terminals. Preserve summary/index byte accounting, path-status counts, and bitmap coverage. Run the existing report-total recomputation after editing the rows. Do not set `state.report.complete=false` merely to signal an incomplete row: the validator uses the collection flag when checking published inferred indexes. [Report invariants](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/inference/mod.rs#L352)

**3.5 Handle the empty inherited array representation explicitly.** The inspected validator has a separate rule for an inferred posting with kind `Array(Unknown)`, normally justified by `report.array_records > 0`. Exercise B with an empty survivor mask and with only B3 remaining. Never fabricate an array record to satisfy that check. If that check rejects a legitimately empty retained posting, add only the narrow proof for this representation: the posting is the existing array/unknown kind, the field report is incomplete, its terminal summary still admits arrays, terminal presence is empty, and value and length memberships are empty. Keep the existing eligibility, domain, report/index consistency and admission checks. Add a negative assertion that a nonempty posting with inconsistent kind evidence still fails. An empty accelerator over an all-missing surviving domain can still prove that a query has zero matches. [Current kind check](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/inference/mod.rs#L415)

## 4. Consolidation must preserve what projection proved

Use the existing disjoint-slot addition and complete-flag propagation in `index/segment/consolidation.rs`. Exact counts from disjoint shards add; valid upper bounds from disjoint shards also add. An incomplete contributing field remains incomplete after a compatible index is merged. Continue deriving missing records from consolidated eligibility and the final present count.

Test a genuinely separate nonempty shard as part of §5. If its empty numeric sibling has `integer_only=None` solely because it has zero surviving numeric values, that sibling contributes no evidence about fractional values. For a merged field still advertised as complete, derive numeric `integer_only` with the same retained-key rule already used by projection if the current merge gives a wrong answer. Preserve `None` where an incomplete contributor could conceal a fractional value. Keep query membership, inferred admission, and active-slot remapping independent of report exactness.

## 5. Regression harness and acceptance assertions

Extend `crates/core/src/index/segment/compaction.test.rs` in the style of its existing fresh-build, routing and validator tests. Use stable IDs and a temporary directory per test. Build each full fixture once. For the oracle, run the ordinary inference builder or a separate repository over exactly the retained records. Feed the original JSON values into the builder. Reusing the production projection to generate the expected counts makes the comparison circular.

1. Enumerate retained masks for A (8 masks), B (16), C (4), the existing nested-name fixture, and the integer/fraction fixture. The three new fixtures give **28 masks** before the existing controls. Include no survivors and all survivors. For a fresh report that omits the target path altogether, its true record and occurrence counts are zero, its missing count is `E`, and it supplies no numeric-type evidence.
2. For each mask, exercise `compact_active` with the complement retired. Exercise `project_inference_for_partitions` with the same retained slots using the existing `PartitionRouting` / `DomainProjection` test pattern. Compare both outputs independently with the fresh oracle. Where a public operation elides an empty part, call the existing projection helper with an empty slot map and verify the empty result at that boundary as well.
3. Consolidate compatible projected shards with disjoint file and record identities. Two parts from a source partition are suitable. Compare against a fresh build of their **union**. Include a real two-nonempty-shard case and an empty contributor. Check a second projection of an already incomplete field so inherited bounds remain safe over repeated operations.
4. At every stage call `state.validate(config, &logical_records, record_count, &schema).unwrap()` on the actual state. Keep the original validator inequalities. For each exact counter assert equality. For each incomplete upper-bound counter assert `fresh <= projected`. For missing assert `projected.missing_records <= fresh.missing_records`. Assert `present + missing == E`, `empty_array <= array <= present`, `empty_selection <= present`, `exact_null <= null_occurrence <= present`, and `nonblank_records <= present`. Include the fixture-specific **exact** assertions in §1 and the previous numeric acceptance checks. Do not compare approximate memory estimates to the fresh oracle.
5. Add a control for withdrawn length coverage: use the existing publication/length-budget fixture or build an equivalent mixed case that independently remains admitted. Establish that the prior report had a terminal empty array and that the projected index lacks complete lengths. Keep a record containing that empty array. Its empty-array and empty-selection bounds must still include one. Keep the existing `items=[]` ancestor-traversal control so an empty ancestor is never counted as a terminal empty array.
6. Add an occurrence control with two nonblank strings selected in one surviving record. The fresh report has two nonblank occurrences, at least one nonblank record and only one eligible record. Exercise the incomplete calculation and assert the occurrence bound includes both.

Use assertion messages containing the fixture name, survivor mask, field path, processing stage and counter. One failure should identify the path through the code and both values without rerunning a debugger.

## 6. Two construction changes

### 6.1 First-batch transfer

Edit `index/posting/mod.rs::PostingCollection::transfer_pristine` and `construction/segment_admission.rs::SegmentAggregate::admit_parallel`. Current code calculates the transferred population before examining `profile`.

- Change `transfer_pristine(&mut self, batch: Self)` to return `Result<(), String>`. Retain its existing compatibility checks followed by `*self = batch`. Return `Ok(())`. The ordinary transfer path does no population counting.
- Inside the existing zero-base/pristine branch in the caller, first run `postings.transfer_pristine(batch_postings)?`. Then, **inside** `if let Some(profile) = profile`, increment the transfer count and obtain the totals from the now-owned destination. Preserve the exact old definitions: keys = number of fields plus all ordinary value-map entries (including a stored Null key) plus all length-map entries. Members = sum of bitmap cardinalities for presence, all values and all lengths, counting each bitmap’s contribution separately.
- Keep counters after successful transfer so a rejected transfer does not count. Leave `postings_pristine`, empty-batch behavior, nonzero-base merging, and final validation intact. The compatibility check may iterate field names with profiling off. The prohibited work is walking dictionary/posting contents merely to collect metrics.
- Update the existing return-tuple test and the public construction fixture. Build the same empty/first/later batches twice, with `None` and `Some(&mut profile)`. Compare path membership, Null, lengths, values, coverage and final statistics. The existing fixture expects **two transfers** because an initial zero-record batch leaves the destination pristine. Verify exact counter totals on a hand-countable batch and that incompatible field sets still return the original error. [Transfer](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/index/posting/mod.rs#L536) · [Caller](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/construction/segment_admission.rs#L429)

### 6.2 Remove the overwritten short-string clone

In `construction/inference.rs::FieldState::observation_copy`, delete the early assignment `copy.short_distinct_without_postings = self.short_distinct_without_postings.clone()`. The function already builds the union from `self.short_values.keys()` and `self.short_distinct_without_postings` and assigns it at the end. Leave that final assignment, including empty-to-`None`, and keep truncation/admission flags unchanged. Test the union with keys `{"A","B"}` from provisional postings and `{"B","C"}` from the independently retained set. The observation must contain `{"A","B","C"}`, the builder must retain its original dictionaries, and two empty inputs must produce `None`. [Exact assignment](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/crates/core/src/construction/inference.rs#L910)

## 7. Verification and delivery

Run these commands from the repair worktree. Use `cargo test -- --list` or the repository’s existing test names to verify each focused filter actually selected tests.

```sh
cargo test -p objekte-search-core --lib --locked projected_ -- --nocapture
cargo test -p objekte-search-core --lib --locked pristine -- --nocapture
cargo test -p objekte-search-core --lib --locked observation_copy -- --nocapture
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
cargo test --workspace --locked --no-fail-fast
git diff --check
```

Record the final code SHA, which baseline assertions failed, the named fixtures and masks exercised, and the workspace test result. Explain any narrowly necessary representation-validation adjustment from §3.5 and identify its negative control. Keep the handoff on the repair branch.

The prior eight-worker Musenalm medians were 887.696 → 887.385 ms with inference disabled and 1,595.941 → 1,561.403 ms with AutoIndex. AutoIndex peak RSS was 894.9 → 869.2 MiB. The profiled run recorded 15 transfers and 410,403 bitmap members. Those figures are background, not a timing target. The mandatory evidence for this repair is correct survivor reports and absence of telemetry traversal on ordinary transfers. If the same corpus/profile is rerun, reconcile its transfer totals against the earlier result. Make a new speed claim only from new alternating, uninstrumented measurements. [Recorded measurements](https://github.com/Simon-Martens/objekte/blob/514d46bdfa/docs/ai/generated/projected_inference_and_construction_repair_results.md#L70)

## Implementation report

Task identifier: `projection-counter-invariants-and-transfer-costs`

Completion date: 2026-11-20

Base commit: `514d46bdfa8af61d94549e202b2f71e16d4085ee`

Final code commit: `f19d5c4370`

Branch: `repair/path-posting-query-evaluation`

Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

The task explicitly directed continuation on the existing repair branch and
worktree, so no separate integration worktree was created.

### Reviewed-revision failures

- A `surviving_empty_arrays_do_not_retain_retired_empty_array_counts`:
  `empty_array_records` was 2; expected 0.
- B `mixed_training_shapes_retain_projected_shape_bounds`: `scalar_records`
  was 0; expected 1.
- C `report_only_projection_preserves_inherited_presence`: `present_records`
  was 0; expected 1.

### Implemented changes

- `finish_projected_inference` now assigns presence, scalar, object, array,
  empty-array, empty-selection, null, nonblank, occurrence and coverage fields
  in the specified dependency order from the surviving presence and terminal
  summaries.
- General projected rows are explicitly incomplete and preserve occurrence
  upper bounds in a nonempty domain. The exact single-valued scalar branch
  remains exact, including numeric `integer_only`.
- Consolidation recomputes complete numeric `integer_only` from merged
  surviving keys, so an empty numeric sibling does not erase evidence from a
  nonempty complete sibling.
- The validator accepts `Array(Unknown)` with empty presence, values and lengths
  only when the incomplete field still admits arrays. A nonempty inconsistent
  posting remains rejected.
- Transfer counters are calculated only under profiling and only after a
  successful ownership transfer. Ordinary transfers walk no dictionary or
  posting contents for telemetry.
- The overwritten short-string clone was removed; the final union assignment is
  unchanged.

### Fixtures and masks

The exhaustive suite ran all 36 masks for A (8), B (16), C (4), the nested-name
fixture (4), and the integer/fraction fixture (4), including empty and complete
survivor sets. Every mask was checked through compaction, partitioned
projection, and a second projection. Fresh builds over the exact retained files
provided the oracle.

Consolidation controls covered two disjoint nonempty mixed-shape shards and an
empty contributor. A separate empty-first numeric consolidation checked
`integer_only = Some(true)`. Additional controls covered withdrawn length
coverage, two nonblank values in one survivor, and the empty
`Array(Unknown)` validator proof with a nonempty negative control.

### Commits

- `d250572176` corrects projected survivor inference bounds.
- `f19d5c4370` keeps pristine transfer telemetry profiling-only and removes the
  overwritten short-string clone.

### Validation

- `cargo test -p objekte-search-core --lib --locked projected_ -- --nocapture`:
  11 selected tests passed.
- `cargo test -p objekte-search-core --lib --locked pristine -- --nocapture`:
  two selected tests passed.
- `cargo test -p objekte-search-core --lib --locked observation_copy
  -- --nocapture`: one selected test passed.
- Core library suite: 660 passed, 16 ignored.
- `cargo test --workspace --locked --no-fail-fast`: passed across CLI, desktop,
  core, integration tests, and doctests.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo check -p objekte-search-core --all-targets --features
  detailed-profiling --locked`: passed.
- `cargo clippy --workspace --all-targets --locked`: completed without errors
  and retained existing warning-level diagnostics.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

### Benchmark reconciliation

The existing eight-worker ASCII Musenalm profile configuration was rerun once
with detailed profiling. It recorded 15 zero-base transfers, 19,565 transferred
keys, and 410,403 bitmap members, exactly matching the prior profile. The
instrumented build took 929.447 ms. This run is explanatory only and is not used
for a new speed claim.

No subordinate worktrees were created. The repair branch and worktree are
retained for the user's final merge.
