# Repair the projected exact-null count after inference stops early

Apply this repair in the existing `repair/path-posting-query-evaluation` worktree. Reviewed revision `f5c9f792475a`. The deliverable is one production assignment and a focused regression test for a single segment.

## Defect and expected behavior

`exact_null_records` counts records for which the path selected exactly one value and that value was JSON null. `null_occurrence_records` also includes records containing null inside a selected array. Every exact-null record is a null-occurrence record.

When inference stops at a budget limit, later records still count toward eligibility. Their values were never observed. If the builder saw zero exact-null records before it stopped, a surviving unobserved record can still have `{"x":null}`. Today projection publishes zero as the upper bound for that survivor. The existing validator accepts zero because it checks only that the exact-null count fits within the null-occurrence count. Verify the result against a fresh observation of the survivor. [Observer](https://github.com/Simon-Martens/objekte/blob/f5c9f792475a/crates/core/src/construction/inference.rs#L1591) · [Validator](https://github.com/Simon-Martens/objekte/blob/f5c9f792475a/crates/core/src/inference/mod.rs#L352)

## Production edit

In `crates/core/src/index/segment/projection/inference.rs`, open `finish_projected_inference`. Find the branch for a field with no retained posting. Immediately after assigning `field.null_occurrence_records`, replace the assignment to `field.exact_null_records` with the following. [Exact location](https://github.com/Simon-Martens/objekte/blob/f5c9f792475a/crates/core/src/index/segment/projection/inference.rs#L283)

```rust
field.exact_null_records = inherited(old_exact_null_records, field.null_occurrence_records);
```

The local `inherited` closure already uses `state.report.complete`, which describes observation of the source domain. The projected row has `field.complete = false` in both cases below.

| Source observation | Required exact-null upper bound |
| --- | --- |
| Finished | The smaller of the old exact-null count and the projected null-occurrence count. |
| Stopped early | The projected null-occurrence count. This is the tightest bound available from these two counters. |

An empty surviving domain yields zero through the same expression. Both ordinary and partitioned projection already call this function.

## Regression test

Add `projected_exact_null_bound_after_observation_budget_exhaustion` to `crates/core/src/index/segment/compaction.test.rs`. This module already imports `project_inference` and `SlotMap`. Use a real `InferenceBuilder` and call the projection entry point directly so batching and filesystem enumeration cannot alter the order. [Projection entry point](https://github.com/Simon-Martens/objekte/blob/f5c9f792475a/crates/core/src/index/segment/projection/inference.rs#L61)

1. Create a default schema and a default `InferenceConfig` with mode `ReportOnly`. Set `config.policy.max_record_contributions = 2`. Create `InferenceBuilder::for_collection(config, 1).unwrap()`. Feed these values through `observe_record` in slot order, using `Value::from_json` and `RecordSlot`.

   | Slot | JSON | Purpose |
   | --- | --- | --- |
   | 0 | `{"x":1}` | The root and its child use two visits. The builder records path `x` with zero exact nulls. |
   | 1 | `{"stop":{"nested":0}}` | The third visit exceeds the per-record limit and interrupts observation. |
   | 2 | `{"x":null}` | This record is eligible and its value goes unobserved. |

2. Finish with logical slots `{0,1,2}`, record count `3`, and the same schema. Assert that `source.report.complete == false`, eligibility is three, the `x` row exists with `exact_null_records == 0`, and there is no posting for `x`. These preconditions establish that the test reaches the defective branch.
3. Keep only old slot 2. Use `SlotMap::Dense(&[None, None, Some(RecordSlot(0))])` in the call, destination logical bitmap `{0}`, destination count `1`, and bounds `None`. Call `project_inference` on `Some(source)` and unwrap both result layers.
4. Build an independent one-record reference by feeding only `{"x":null}` at slot 0 to a fresh builder with the same config. This record fits the budget. Assert that its report is complete and its `x` row has exactly one exact-null record.
5. Validate the projected state with `validate(config, &destination_logical, 1, &schema)`. Assert that its report and `x` row are still incomplete, the row's null-occurrence bound is one, and its exact-null bound equals the reference value of one. The last assertion must fail with actual zero on the reviewed revision.

Include one small control in the same test. Use `ReportOnly` with the default contribution budget, observe only `{"x":[null]}`, and project that one slot to itself. Assert the source report is complete and the projected row has `null_occurrence_records == 1` and `exact_null_records == 0`. The array contains null, but the selected value is the array. This catches an implementation that always assigns the null-occurrence bound.

## Verification and handoff

Run the named test before editing the assignment, then rerun it after the edit. Run the existing projection tests and formatting check.

```sh
cargo test -p objekte-search-core --lib --locked projected_exact_null_bound_after_observation_budget_exhaustion
cargo test -p objekte-search-core --lib --locked projected_
cargo fmt --all -- --check
```

Report the failing and passing counts for the one-survivor fixture and the outcome of the control. This repair concerns diagnostic metadata for one projected segment and requires no construction benchmark or segment consolidation work.

## Implementation report

Task identifier: `projected-exact-null-bound-repair`

Completion date: 2026-11-20

Base commit: `f5c9f792475a`

Final code commit: `d2f3c8a3a8`

Branch: `repair/path-posting-query-evaluation`

Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

The task explicitly directed continuation in the existing repair worktree, so
no separate integration worktree was created.

### Implemented change

The no-posting projection branch now derives `exact_null_records` through the
existing `inherited` closure. A completed source observation retains the
tighter old-count bound. An observation that stopped early uses the projected
null-occurrence bound, which is one in the unobserved surviving-null fixture.

### Regression result

Before the production edit, the focused test failed on the required final
assertion with projected exact-null count `0` and fresh survivor reference `1`.
After the edit:

- The one-survivor fixture passed with projected exact-null count and reference
  count both equal to `1`.
- The projected report remained incomplete.
- The array-element-null control passed with null-occurrence count `1` and
  exact-null count `0`.
- Empty-domain behavior remains zero through the same expression.

### Validation

- `cargo test -p objekte-search-core --lib --locked
  projected_exact_null_bound_after_observation_budget_exhaustion`: one passed
  after the baseline one failure.
- `cargo test -p objekte-search-core --lib --locked projected_`: 12 passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

This diagnostic-only repair required no construction benchmark or segment
consolidation work. The repair branch and worktree are retained for the user's
merge.
