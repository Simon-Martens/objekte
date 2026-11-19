# Assignment 5 repair addendum — Relation sort selectors and incomplete discovery

**Branch** `integration/core-query-and-lifecycle-repairs`
**Reviewed branch head** `b9e92cd63bb4828891c4218ee0c748b9d06ace21`
**Last tested code commit** `8ee3cb1fdddfa9ccd4c25c95149f2ba8a5a46e10`
**Original master base** `601a8e597513701d5933094eece7b0ad9ce555e2`

Continue the existing integration worktree. The reviewed tip includes the completion report and the Musenalm benchmark artifacts. Preserve them. Fix the two cases below before handing the branch back for merge and Assignment 7. The evidence below is a source trace and a checked cache-size calculation. Run each new regression before and after its fix and record the actual output.

## 1. Honor wildcard occurrence selectors on one-hop shortcuts

A scalar forward relation has an empty occurrence-position vector. The binder encodes `[*]` as one `None` entry in the selector vector. The established `positions_match` predicate first requires enough actual positions. For a scalar edge, `[None]` fails that length check. The optimized helpers at `crates/core/src/query_results/ranking.rs:552` and `:619` currently reject only selectors containing an explicit index. They accept the wildcard and discard the available edge positions.

1. Keep both helpers limited to forward, to-one hops. The conservative eligibility rule for the repair is an **empty selector vector** wherever a fast path reuses a key without checking the actual occurrence positions. Fall through to normal traversal for every explicit array step. Preserve current collation checks, target projection checks and reuse for ordinary selector-free hops.
2. Check `ComputedSort::direct_related_target_column` and the matching direct relation page path in `query_execution/runner/relations.rs`. Apply the same rule wherever the page path reuses a related sort key without exact positional proof. Check the corresponding relation predicate/FTS page eligibility if that shortcut can declare an exact answer from reverse postings without verifying the same selectors. Candidate-only pruning may retain a superset when exact verification still follows.
3. Add a public pinned regression with two sources, `S1 → T1` and `S2 → T2`, each with a scalar to-one `entry`. Set stored target titles to `Zulu` and `Alpha` respectively, with compatible collation. Disable the result cache. For `SORT BY entry[*].title ASC` both keys are missing. Expected IDs are `[S1, S2]` by the existing ID tie-break, first page `[S1]`, count 2. For `SORT BY entry.title ASC` expect `[S2, S1]`, first page `[S2]`. Assert the optimized and reference public answers independently. Verify the compatible selector-free query still has zero sort-target reads.
4. Exercise the stored-key and cached-filter helpers, and any direct-page shortcut modified above. Use the existing field-to-field projection fixture or owner-local assertions to prove the cached-filter helper declines. Include a genuine array-occurrence control so a valid wildcard still traverses its matching positions. Confirm that explicit positional backlinks and numeric local/related collation still pass.

[Ranking eligibility](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/query_results/ranking.rs#L549) · [Binding of wildcards](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/query/binding/relation.rs#L510) · [Exact selector semantics](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/relation_query.rs#L2284)

## 2. Stop reference ranking when discovery makes the key unverifiable

At `crates/core/src/query/sort_binding.rs:882`, the code sets `unverifiable` and still iterates through the known sources, charges edges, reads targets and evaluates the reducer. Any resulting error propagates before the caller checks the flag at line 579. This allows discarded work to turn a specified partial result into a query-wide error.

1. Preserve the discovery read, byte accounting and original source diagnostic. As soon as an incomplete discovery is observed, return the existing unverifiable outcome from this traversal. Propagate it through recursive callers and stop siblings, reducers and remaining terms for the affected row. Keep processing independently rankable rows. Keep genuine errors and resource limits on work that is still required; changing the cache limit or suppressing its error globally would hide the defect. Preserve known witnesses for FILTER/FTS and the existing per-relation discovery cache.
2. Extend the pinned backlink regression. Use one target A, eight distinct readable logical source records referencing A via `contributors`, and one additional valid source file. Pad each readable linked JSON record to exactly 2 MiB with spaces **inside the object before the closing brace**, so the stored record range includes the padding. Remove the ninth file after pinning. Query target A with `SORT BY MIN(@backlinks.contributions.title)` and request the count. All eight known sources are relevant to the old traversal. Their estimated charge is `8 × (4 × 2,097,152 + 128) = 67,109,888` bytes, which exceeds the 64 MiB cache by 1,024 bytes. Discovery itself is streamed.
3. On the reviewed code, record the actual failure. After repair, require a successful search returning no hits, `count=None`, `total_matches=0`, incomplete match count, and the diagnostic for the missing ninth file. Require zero sort-target reads for these discarded rows. Use a second dependent target or the existing owner-local lookup test to prove cached incompleteness does not repeat discovery. Keep the independent-relation control, the healthy three-read backlink control, and the genuine sort-budget failure test.
4. Keep test instrumentation opt-in. The current test-only row and range observers append owned IDs/paths during every test-thread invocation, even outside the observing fixture. Arm each observer only around the measured query, drain its observations and disable it on scope exit, including unwind. Ordinary test and ignored benchmark runs should incur no per-record observation allocations outside that scope.

[Incomplete lookup handling](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/query/sort_binding.rs#L872) · [Target-cache charge](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/query/sort_binding.rs#L172) · [Existing fixture](https://github.com/Simon-Martens/objekte/blob/b9e92cd63bb4828891c4218ee0c748b9d06ace21/crates/core/src/relation_query.test.rs#L2092)

## Acceptance and delivery

Use the common specification in `docs/ai/done/objekte_core_assignments_4_5_6_repair_specification.md` for final gates. Run the new regressions individually with positive counts, the existing related-sort and partial-discovery controls, all four A4 controls, the concurrent shutdown regression, workspace formatting/check/tests, all-feature core tests, documentation, the focused lifecycle suite, and the ignored release compaction regression at its default 262,144 records with two attempts. Record exact commands, baseline failures, final IDs/counts, physical reads and the actual tested code SHA. Commit and push the code and completion report on the integration branch, identify the full delivery SHA, and return it for owner review.

## Implementation report

- **Task:** `assignment-5-relation-sort-repair-addendum`
- **Completion date:** 2026-11-19
- **Reviewed starting HEAD:** `b9e92cd63bb4828891c4218ee0c748b9d06ace21`
- **Original master base:** `601a8e597513701d5933094eece7b0ad9ce555e2`
- **Integration branch:** `integration/core-query-and-lifecycle-repairs`
- **Integration worktree:** `/home/simon/source/objekte-integration-core-query-and-lifecycle-repairs`
- **Tested code revision:** `93ef3c47c6891d2a46e7be0de4632ff425cb3336`
- **Implementation commit:** `93ef3c47c6891d2a46e7be0de4632ff425cb3336`
- **Status:** complete and ready for owner review; no merge into `master`

### Implemented

Fast one-hop related sort and relation-page reuse now requires an empty source
selector vector. A `[*]` selector therefore falls through to normal traversal,
where `positions_match` applies the established occurrence semantics. This rule
is applied in the two merged-ranking sort helpers, direct target-column
eligibility, exact relation target pruning, the direct relation page, and exact
relation-FTS semijoins. Selector-free stored-key reuse and candidate-only
superset pruning retain their existing behavior.

Reference ranking now returns immediately when streamed backlink discovery
marks the lookup incomplete. The discovery read, byte accounting and missing
source diagnostic remain recorded, but no known source is traversed, no target
is decoded, no reducer is evaluated and no later sibling or sort term can turn
the unverifiable row into a query-wide target-cache error. Independently
rankable rows still create fresh traversal state and are processed normally.

The reference row and search-range observations are now opt-in RAII scopes.
Their thread-local storage starts disabled, records nothing outside an armed
scope, drains and disarms on `finish`, and also disarms during unwind.

### Regressions

`relation_query::tests::scalar_relation_wildcards_do_not_reuse_selector_free_sort_keys`
is the pinned public regression. Before the fix it returned `["m2", "m1"]` for
`SORT BY entry[*].title ASC`; after the fix it returns `["m1", "m2"]`, with the
first page `["m1"]` and count 2. `SORT BY entry.title ASC` remains
`["m2", "m1"]`, with first page `["m2"]`, count 2 and
`sort_target_reads=0`.

The same test proves both optimized helpers return `None` for the wildcard,
compares optimized and reference relation FILTER/FTS paths, and exercises a
genuine `MIN(items.entry[*].title)` array control. Existing explicit positional
backlinks, related reducers and numeric related/local collation controls pass.

`relation_query::tests::incomplete_backlink_discovery_stops_before_target_sort_cache_work`
uses one target, eight distinct 2 MiB linked logical sources and one additional
source removed after pinning. On the reviewed behavior it failed with:

```text
called `Result::unwrap()` on an `Err` value: "sort target cache budget exceeded"
```

The old retained charge is `8 * (4 * 2,097,152 + 128) = 67,109,888` bytes,
exactly 1,024 bytes above 64 MiB. After the fix the public query returns no
hits, `count=None`, `total_matches=0` and an incomplete match count. It reports
the missing `mentions/ninth.json` diagnostic. Work counters are:

```text
sort_target_reads=0
relation_source_reads=8
relation_bytes_read=16,777,216
files_opened=9
record_parses=9
total bytes_read=16,777,243
```

An owner-local request against the same pinned view also returns no verified
key, reads the eight linked sources once, charges 16 MiB and performs zero
target-cache reads. The existing multi-target lookup test confirms cached
incompleteness is reused without another discovery scan.

### Validation actually run

All gates were run at tested code revision
`93ef3c47c6891d2a46e7be0de4632ff425cb3336`:

```text
cargo fmt --all -- --check                                      passed
cargo check --locked --workspace --all-targets                  passed
cargo test --locked --workspace                                 passed
cargo test --locked -p objekte-search-core --all-features       passed
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc \
  --locked --workspace --no-deps                                passed
cargo test --locked -p objekte-search-core --lib lifecycle::    passed
```

The default core unit suite reported 605 passed, 0 failed, 16 ignored. The
all-feature core unit suite reported 606 passed, 0 failed, 16 ignored. The
focused lifecycle suite reported 89 passed, 0 failed, 6 ignored. Default and
all-feature inventories are 621 and 622 entries, exactly two more than the
reviewed head.

The ignored release compaction regression was rerun with its environment
override unset:

```text
large compaction build records=262144 elapsed=257.476371ms
large compaction records=262144 elapsed=264.436122ms attempts=2
  before=262146 after=262145 retired=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 620 filtered out
```

The new wildcard and incomplete-discovery regressions each reported one passed.
The complete `relation_query::tests::` group reported 33 passed. The complete
`query::sort_binding::tests::` group reported 12 passed. The three
`base_reference_` controls, segmented FIRST/LAST control, incomplete-filter
projection control, forward-source accounting control, healthy backlink and
multi-target discovery controls, genuine target-budget failure control,
concurrent shutdown regression, and all four A4 controls each reported positive
execution.

No separate wall-clock benchmark applies to these correctness fixes. The
relevant evidence is the exact read, byte, target-cache and occurrence-selector
behavior above.

### Handoff

The implementation diff is confined to the relation sort, relation execution,
and test-observation paths. The completed task document is committed as the
report-only commit following the tested code revision. No unresolved issue
remains. The integration worktree and branch are retained for owner review.
