# Objekte — segmented inference and posting cost corrections

**Reviewed branch** `repair/path-posting-query-evaluation` at `71d64f7bcf`. The last code revision is `c441d43d6e`. The original integration branch remains at `b3f7e9dc9c`. Apply the follow-up on the repair branch or a worktree based on it. The user will merge the result.

The terminal-wildcard and segment-local facet fixes are sound in the reviewed paths. Keep their public regression tests, restored null admission, selected-null storage, and inferred facet presence optimization. Three small areas still need changes. The first two affect inferred index availability or the validity of a projected state. The third affects plan selection. These findings come from tracing the repaired code and its tests. No Rust toolchain was available for an independent test run.

## 1. Count distinct array lengths across segments

Collection inference adds each segment’s number of distinct lengths. It then compares that sum with `max_distinct_values_per_path`. Two segments containing only arrays of length 1 contribute two to the sum. Their union has one key. With a budget of one, publication can discard valid length postings and send a previously covered query to the source. Changing only segment boundaries can therefore change index admission and query I/O. The new regression uses disjoint lengths, for which the sum happens to be correct. [Aggregation](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/construction/inference.rs#L596) · [Admission decision](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/construction/inference.rs#L313) · [Publication](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/lifecycle/publication.rs#L780) · [Existing disjoint-length test](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/inference/tests.rs#L671)

- Compute a bounded union of observed length **keys** for each logical path. Reuse the already available ordered length dictionaries when they completely cover the observations. If the decision also needs observations without a published dictionary, retain only the bounded unique lengths needed for that decision. Keep local overflow or incomplete evidence explicit. Stop once one more distinct key than the limit is established. No record bitmaps or source reads are needed for aggregation.
- Keep physically stored per-segment entry counts separate from a cardinality used for admission. Use a metric labeled “distinct lengths” only for a deduplicated count.
- With auto-indexing and a limit of one, build the same numeric-array records as one segment and as at least two contributing segments. All arrays have length 1. Assert identical admission, complete length coverage, reference IDs and zero predicate source reads for `x:length = 1`. Verify the fixture really created multiple observations. Then change one length to 2. The global length capability must be withdrawn and the public query must still agree with the reference. Keep the existing disjoint-key case.

## 2. Make projected reports valid and honest about exactness

**A reproducible validation failure.** Build five logical records in one segment. Two have `x:[null]`, one has `x:[7]`, and two omit `x`. Retire the two null-array records and compact. The three survivors have one record containing `x` and zero observed null occurrences. The projection computes presence as 1 and carries forward `min(2, 3) = 2` null-occurrence records. The production validator explicitly rejects `null_occurrence_records > present_records`, including for incomplete reports. The current test only requires the null count to fit the total eligible population. [Projection](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/segment/projection/inference.rs#L107) · [Validator](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/inference/mod.rs#L352) · [Existing test](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/segment/compaction.test.rs#L278)

**Two further contradictions in the same routine.** For a single-valued scalar path, projection can leave `field.complete` true while unconditionally setting `value_occurrences` to zero. Retaining even one numeric value disproves the count. For a nested array path with both empty traversals and terminal empty arrays, `lengths[0]` covers only the latter. Take `{"items":[]}` and `{"items":[{"x":[]}]}`: the fresh inference report for `items.x` counts two empty selections. The projection’s length-first branch counts one. That is a lower bound, whereas the repair report describes conservative upper bounds for unreconstructable counts. [Empty-selection calculation](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/segment/projection/inference.rs#L117) · [Counter resets](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/segment/projection/inference.rs#L184)

- For array paths, bound inherited null-occurrence counts by the projected possible-present population as well as the source count. Keep provable selected nulls as a lower bound. Preserve the invariant `exact_null_records ≤ null_occurrence_records ≤ present_records ≤ eligible_records`. Maintain the distinction between an element null and a terminal null in public queries.
- Use length zero as an exact empty-selection count only where empty ancestor traversals cannot also contribute. For an ambiguous union, use a valid, documented bound and mark the field incomplete. Never add two overlapping record counts as though the records were disjoint.
- Review every counter written by this projection before setting `field.complete`. Derive exact values where cheap. When any count is unknown, mark the field incomplete and document the convention used by its numeric fields. Preserve independently proved query coverage. Carry incompleteness across consolidation, as the repair already does.
- Include one string fixture where retiring the last covered string leaves only missing records. Projection currently retains the path’s empty short-string coverage entry while setting its distinct count to zero. The validator requires a positive distinct count whenever the entry is present. Make the resulting capability representation consistent with the production validator and preserve query fallback for genuinely unresolved records. [Projection of coverage entries](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/segment/projection/inference.rs#L82) · [Coverage validation](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/inference/mod.rs#L371)
- Run `InferenceState::validate` on the actual projected and consolidated states from these fixtures. For every field advertised as complete, compare the occurrence counters against a fresh build of exactly the surviving records. For incomplete fields, assert the documented bounds against that same oracle. A posting-only validation or a check against total eligible records misses the primary failure.

## 3. Remove the cost discontinuity at the probe limit

When 32 keys match, the exact and candidate estimators both charge 32 units of lookup work. At 33 matching keys they switch to `(all value keys + all length keys) / 8`. With 33 strings and a single null key the estimate drops to 4.25, although the executor reads one more matching posting. Unrelated keys can also raise the estimate for a typed range. Result membership is unaffected, but the planner receives a reversed cost signal at the threshold. The existing 100-key test only asserts that the estimate exceeds one. [Exact estimate](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/posting/lookup.rs#L402) · [Candidate estimate](https://github.com/Simon-Martens/objekte/blob/71d64f7bcf/crates/core/src/index/posting/lookup.rs#L435)

Use the number of matching keys already observed as a floor for the same operation’s estimate. Share the threshold calculation between exact and candidate costing. Use the relevant dictionary size when readily known. Keep the existing cap on estimator traversal and avoid constructing a bitmap union during planning. Reuse the multivalued nullable-string fixture for 31, 32, 33, and 100 matching keys. Check that exact and candidate lookup work never decreases as matching keys are added and that probe work remains bounded.

## Acceptance

Run the new focused fixtures against the reviewed code to capture the failures, then against the fix. Run the workspace formatting, check and test commands already used on this branch. Retain the existing Q203 zero-read check. The new length test’s I/O counters and the production validator on compacted data are the decisive evidence. A repeat of the entire Musenalm benchmark suite is unnecessary for these corrections. Record the tested SHA and outcomes on the repair branch.

## Implementation report

- Task identifier: `segmented-inference-posting-cost-repair`
- Completion date: 2026-11-20
- Review base: `71d64f7bcfd5a50b9e556e3523a5fe3d3de13f69`
- Branch: `repair/path-posting-query-evaluation`
- Worktree: `/home/simon/source/objekte-repair-path-posting-query-evaluation`

### Implemented corrections

Collection inference now retains a bounded, deduplicated set of observed length
keys for each path. Segment observations move their ordered length keys into
the construction summary, and collection aggregation inserts each key once,
stopping as soon as one key beyond the configured limit is found. Physical
per-segment entry totals are no longer used as a global distinct-length count.
The existing disjoint-key case remains a withdrawal case.

Projected inference reports now preserve the production validator invariants:

- `exact_null_records <= null_occurrence_records <= present_records`.
- Missing records remain the complement of the reported present upper bound.
- Numeric fields in incomplete reports use conservative upper bounds and are
  marked `complete = false`.
- Terminal zero-length arrays contribute an exact empty-selection count only
  when no empty ancestor traversal can overlap. Ambiguous ancestor/terminal
  unions retain the larger inherited upper bound.
- Empty projected short-string coverage entries are removed, so a zero
  distinct count never remains paired with a present coverage capability.
- Consolidation continues to carry contributor incompleteness.

Both projected and consolidated states from the new fixtures are passed through
`InferenceState::validate`; posting-only validation is not used as the
correctness proof.

Exact and candidate estimators now share a monotonic probe-cost calculation.
Observed keys below 33 cost their exact probe count. At the cap, the estimate
is floored at 33 and grows only when a relevant homogeneous dictionary size is
readily available. Mixed typed ranges do not inherit unrelated dictionary keys.
The 31/32/33/100-key fixture asserts nondecreasing exact and candidate work and
bounded probe visits.

### Contributing commits

- `4f328a28fe` — Deduplicate inferred length keys across segments
- `ea3b4cbfdd` — Make projected inference reports valid and bounded
- `bfa53459b1` — Keep posting probe costs monotonic

### Validation

- `cargo fmt --all -- --check` — passed.
- `cargo check --workspace --all-targets` — passed.
- `cargo test --workspace` — passed. Core: 647 passed, 16 ignored; all CLI,
  desktop, integration, and documentation tests passed.
- Existing terminal-wildcard, delta-slot, null admission, selected-null, and
  inferred facet-presence regressions remain green.
- Release `verify_structural_reference` was run for Q106, Q107, Q195, Q196,
  Q198, Q203, Q207, Q208, Q218, Q219, and Q220. Counts and page hashes match
  the reviewed revision and source reference. Every case reports zero files
  opened, bytes read, record parses, and scanned records, including Q203.

### Length admission evidence

The focused fixture verified two actively contributing segment observations.
With all arrays of length one and a one-key budget:

- The one-segment and multi-segment builds both remain `Built`.
- Length coverage is complete.
- `x:length = 1` returns reference-identical IDs.
- Predicate evaluation reports zero scanned records and zero record parses.

Changing one array to length two establishes the union `{1, 2}`. The global
length capability is withdrawn as `BudgetExceeded`; the public query remains
reference-identical and performs residual source verification. The earlier
disjoint-length publication regression also remains green.

### Projected report evidence

The five-record null fixture retires both `x:[null]` records and leaves one
`x:[7]` plus two missing records. The projected field reports presence 1, null
occurrences no greater than presence, and zero terminal selected nulls.
`x:null = true` is empty, and both projected and consolidated states pass
`InferenceState::validate`.

The nested fixture distinguishes one successful empty ancestor traversal from
one terminal empty array. The fresh report has two empty selections while the
length-zero posting contains one. Projection retains the valid upper bound 2,
marks the field incomplete, and passes production validation.

The string fixture retires the only covered string and leaves a missing record.
The visible empty short-string coverage entry is removed, its distinct count is
zero, and the projected state validates.

### Cost evidence

The deterministic multivalued nullable-string fixture shows monotonic exact and
candidate work for 31, 32, 33, and 100 matching keys. Estimator traversal stays
at or below 33 keys, and no planning-time bitmap union is constructed. A broad
100-key operation costs more than the one-posting proof, and restricted
execution still returns only the requested input intersection.

No new wall-time benchmark was run because this follow-up does not alter the
underlying data layout and the specification explicitly waived a full
Musenalm benchmark repeat. The decisive evidence is the focused query I/O,
validator, and deterministic planner-cost assertions above.

### Remaining limitations

- Incomplete projected reports use upper bounds for occurrence-style counters
  and the complementary present/missing counts. Their fields remain explicitly
  incomplete, so these values are not advertised as exact.
- Typed mixed dictionaries without a homogeneous size proof retain the
  33-probe floor rather than estimating unrelated keys.

No unresolved in-scope defects are known. No subordinate worktrees were
created; the repair branch and worktree remain for user review.
