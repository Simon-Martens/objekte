# Assignment 2 — Final diagnostic and test corrections

**Branch** `integration/core-index-construction`  
**Reviewed delivery** `c8ac4bab646eeed6b14209b6c59e21e95f959823`  
**Reviewed implementation** `2bb6a9cb774e6df81f749e36d2175a3bd7d6ad20`

The canonical build API, benchmark owner lifetime, immediate ownership errors, segment-limit validation, test registration and construction split are in place. Preserve them. The remaining changes concern two newly exposed diagnostic methods and one test whose exact result depends on worker teardown. Continue on the existing integration branch. [Repair delivery](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/docs/ai/done/objekte_core_construction_review_repairs.md)

## 1. Give diagnostic counts a single revision

`IndexView::construction_observation` iterates the pinned catalogue for file counts and bytes, reads record counts from the original repository collection generation, and examines currently published segments for physical counters. After building one file with one record and adding another such file, a new view can report two files and one record. The old view still reports one and one. The current implementation also walks the catalogue once per collection, collects full vectors of borrowed rows solely to count and sum them, then walks the catalogue twice more for totals. [Observation method](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/src/lifecycle/view.rs#L110) · [How adding a file publishes the new catalogue and segments](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/src/lifecycle/mod.rs#L1533)

- Make the logical counts describe the pinned publication. Accumulate files and bytes in one traversal of active catalogue rows. Count each physical file once in the total, and once per collection membership in the per-collection counters. Accumulate live records per collection from its base liveness mask and published segment liveness masks, respecting the existing representation. Completed native builds also retain metadata rows for the published records. Counting those rows in addition to segment liveness would double count.
- Group physical segment counters once by collection ordinal. Allocation capacities, heap sizes and stored postings are physical measurements and may include retired entries until compaction. Distinguish them in the type or documentation from live records. Label statistics derived from the original build-time metadata accordingly. Keep existing benchmark JSON keys where they already carry the specified meaning. A name and doc comment that describe this as a pinned observation should replace the current claim that every field describes the original construction generation.
- Each aggregate needs at most one accumulator per collection. Do not materialize catalogue rows or query-result memberships to compute it. Reuse any summaries already obtained by the calling benchmark where practical.
- Add a small public-path test. Pin a one-file, one-record completion, add a second one-record file, and then delete the original. The three retained views must independently report logical file/record counts of 1/1, 2/2 and 1/1. Verify per-collection counts and the corresponding paths. Include one shared file in two collections if existing coverage does not already establish the global-file versus membership rule.

## 2. Select a live record for the lookup benchmark

`record_lookup_observation` picks the first original construction record. If that file was deleted, the `?` on its catalogue lookup returns `None` immediately even if another record is live. If a file is replaced under the same file ID with a different logical record ID, the method can return an ID that no longer resolves. Its consumer uses that ID to benchmark record lookup. [Selector](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/src/lifecycle/view.rs#L224) · [Consumer](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/examples/bench_index_construction.rs#L543)

Move the sample selection to the benchmark, its only implementation consumer on the reviewed revision. Remove the newly added public `RecordLookupObservation` and method. Before the timed loop, select one visible hit through the public query path on the pinned view, with locator return mode, evidence disabled and a limit of one. Use that hit’s collection, record ID and file information for the timed lookup on the same view. Handle the empty selection exactly as the benchmark requires. Keep sample selection and source materialization outside the timed lookup. Exercise deleting the first source file while a second record remains and verify the sampled ID resolves on that view.

## 3. Make the ownership regression deterministic

`repository_conversion_rejects_retained_owner_and_view_promptly` builds through the public asynchronous task, then asserts the exact ownership layer that failed. A successful `wait_complete()` returns the published index while the worker can still retain its task state. In the retained-view case either the index or the publication can therefore be the first shared layer. Both errors are legitimate, but the test asserts only the latter. This race is separate from the isolation-sensitive executor assertion noted in the completion report. [Test](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/src/lifecycle_initial.test.rs#L223) · [Wait semantics](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/crates/core/src/lifecycle_initial.rs#L258)

Construct each index for this specific test using the existing synchronous, test-only `Index::build_repository_for_test` fixture followed by `Index::from_repository`. Then deliberately retain only an `Index` clone for the first case and only an `IndexView` for the second. Keep the exact typed assertions and retain a successful exclusive-adoption test. No sleeps, retries, worker joins in production, or change to the public completion contract are required. Keep end-to-end tests of public build progress and repeated waits on `Index::start_build`.

## Delivery evidence

Run the new diagnostic cases and the deterministic conversion regression. Run the two restored admission-accounting tests with `--features detailed-profiling`, then the core and workspace checks used for the preceding repair. Smoke-test the changed benchmark with a real locator lookup and one small Musenalm query through cache-off and warm modes. A repeat of the construction timing suite is only warranted if these changes alter the measured build interval. Record the tested implementation SHA and command results on this branch.

The previous same-input comparison matched the schema, signature and 148,486 records/parses. The older committed signature includes different file modification timestamps. Use that same-input comparison as the parity evidence. [Same-input comparison](https://github.com/Simon-Martens/objekte/blob/c8ac4bab646eeed6b14209b6c59e21e95f959823/docs/ai/done/objekte_core_construction_review_repairs.md)

## Implementation report

- Task identifier: `core-index-construction`
- Completion date: 2026-11-18
- Base commit: `c8ac4bab646eeed6b14209b6c59e21e95f959823`
- Integration branch: `integration/core-index-construction`
- Implementation commit: `b906327e54`
- Integration worktree: `/home/simon/source/objekte-integration-core-index-construction`

The pinned observation was renamed to `IndexView::index_observation` and now
uses one active-catalogue traversal for global and per-membership file counts
and bytes. Logical record counts combine each collection's base liveness mask
and published segment masks, without adding retained construction metadata
rows. Physical family counters are accumulated once per collection and are
documented separately from logical values and build-metadata capacity and
identity fields. The public `RecordLookupObservation`, the view-side selector,
and their repository-shaped state access were removed.

`bench_index_construction` now selects its storage sample before the timed
loop through `IndexView::search` with locator output, one-record limit and
evidence disabled. It uses the returned collection, record ID and path on the
same pinned view. A public-path regression selects a live record, deletes its
source while another record remains, and verifies that both the pinned and
current sampled IDs resolve. The ownership regression now builds through the
synchronous test fixture and verifies retained-index, retained-view and
exclusive-adoption outcomes without worker teardown races.

Added diagnostic and locator regressions:

- `lifecycle::tests::index_observation_counts_only_each_pinned_publications_live_records`
- `lifecycle::tests::index_observation_counts_shared_files_once_globally_and_per_membership`
- `lifecycle::tests::benchmark_locator_sample_follows_live_records_after_source_deletion`

### Validation

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed. |
| `cargo check --locked --workspace --all-targets` | Passed. |
| `cargo check --locked -p objekte-search-core --all-targets --all-features` | Passed. |
| `cargo test --locked -p objekte-search-core` | Passed: 580 unit tests, 16 ignored, all integration tests and the doc test. |
| `cargo test --locked --workspace` | Passed on the first run. |
| `cargo test --locked -p objekte-search-core index_observation` | Both new diagnostic tests passed. |
| `cargo test --locked -p objekte-search-core repository_conversion_rejects_retained_owner_and_view_promptly` | Passed. |
| `cargo test --locked -p objekte-search-core benchmark_locator_sample_follows_live_records_after_source_deletion` | Passed. |
| `cargo test --locked -p objekte-search-core --features detailed-profiling large_framed_file_admission_has_component_accounting` | Passed. |
| `cargo test --locked -p objekte-search-core --features detailed-profiling incremental_admission_totals_match_each_collection` | Passed. |

The release `bench_index_construction` smoke run selected the real locator
`agents/009s5q5l5ux57k3` (`data/agents/009s5q5l5ux57k3.json`) through the
public query path. Its measured exact path lookup was 67 ns, record-ID lookup
135 ns, source materialization 1,231 ns, exact path query 84,299 ns and range
path query 12,990 ns. The build completed with 148,486 records and parses.
Timing comparison was not repeated because the repair changes post-build
observation and benchmark sample selection rather than the measured build
interval.

The default `bench_musenalm` Q001 smoke run completed cache-off in 6,150.946 µs
and warm in 58.206 µs. The `--incremental` Q001 run completed cache-off in
6,730.914 µs and warm in 53.815 µs. All four runs succeeded. Temporary
benchmark result directories were removed. No unresolved in-scope failures or
limitations remain; the integration worktree and branch are retained for the
owner's merge.
