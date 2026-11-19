# Objekte core completion · Assignment 3 of 3

## Relation evaluation, crate layout and final integration

**Outcome.** A contributor can locate relation declarations, binding, provider selection, physical edges, pinned lookup and exact evaluation by following their callers. A source-backed reference query still detects a missing or incorrect physical index contribution. Deliver one integration branch with evidence that all three assignments preserve observable results, query work and construction performance.

| Delivery item | Requirement |
|---|---|
| Roadmap coverage | Stage 5 in two commits, Stage 6 in one commit, then final series acceptance |
| Integration branch and worktree | `integration/core-refactoring-completion` at `/home/simon/source/objekte-integration-core-refactoring-completion` |
| **B** | Original series baseline recorded by Assignment 1. The roadmap was reviewed at `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`. Use the actual B for final comparisons. |
| **A2** | Reviewed, pushed Assignment 2 delivery SHA. Start implementation here. |
| **C** and **A3** | C is the final source commit tested and benchmarked. A3 is the subsequent evidence and completion-report delivery commit. Report both full SHAs to the owner. |

Read `AGENTS.md`, `docs/ai/README.md`, `docs/development/code-guidelines.md` and both predecessor reports. Reuse the series branch and sibling worktree, preserve its history, and push each commit. The owner merges the completed branch. Verify ancestry B → A1 → A2. Reuse the test lists and mappings recorded at those exact revisions. Review the Assignment 2 reader/identity matrix, single-pass verification counters and any outstanding issue before attributing a change to relation work.

Paths below are relative to `crates/core/src/` unless they begin with a repository-level directory. At A2, run the relation controls in this document. Keep selected strategy, plan and work observations from representative indexed FILTER, selected-field relation FTS and residual/source-dependent cases. Reuse the deterministic small fixtures in the semijoin, query and target-cache suites. Run identical requests at C with the same workers, cache configuration and publication state.

### Commit 1 · Move relation declarations, semijoins, cache and evaluation intact

Commit subject `Assign relation declarations, provider operations and target caching to their owners`.

| Source on the reviewed base | Destination | Implementation instructions |
|---|---|---|
| `relation.rs` | `schema/relations.rs` | Declare `pub mod relations;` in `schema/mod.rs`. Move `RelationCardinality`, `RelationDefinition`, `RelationId`, the `impl Schema` registration methods and both validation routines together. Retain the same validating calls during field registration and repository initialization. |
| `relation_semijoin.rs` and companion | `query_access/relations/semijoin.rs` and `query_access/relations/semijoin/tests.rs` | The existing file `query_access/relations.rs` declares its child. Move strategies, witnesses, estimates, restrictions, execution and limits together. Keep `NativeRelationProvider` in its present file. |
| `relation_target_cache.rs` and companion | `query_execution/relations/target_cache.rs` and `query_execution/relations/target_cache/tests.rs` | Move `TargetValueCache`, its work accounting, projected-field checks and ordinary/stream preparation intact. |
| `relation_query.rs` and companion | `query_execution/relations/evaluation.rs` and `query_execution/relations/evaluation/tests.rs` initially | Create `query_execution/relations/mod.rs` and declare it crate-private from `query_execution/mod.rs`. Move the full implementation and tests first. Commit 2 extracts two responsibilities from this compiling checkpoint. |

Use ordinary module declarations. Update direct, grouped, aliased and fully qualified imports across the active workspace, examples, integrations and live documentation. Remove the four retired root declarations. Preserve external call syntax for `Schema::relations` and `Schema::register_relation`. The three public types move from `objekte_search_core::relation` to `objekte_search_core::schema::relations`. Keep ordinal assignment, derives, serialized names, reverse-alias validation, implicit indexing of declared ID fields and configuration identity.

Update consumers of the new defining paths, including ranking, sorting, repository configuration, schema JSON compilation, construction and physical indexes. Keep relation binding at `query/binding/relation.rs`, physical edge storage at `index/relation.rs`, graph inspection at Assignment 1's `repository/relations.rs`, and runner orchestration at `query_execution/runner/relations.rs`. Preserve the public receivers of inspection operations. The mechanical move is reviewable with rename detection and a check of the remaining non-import diff.

### Commit 2 · Separate the pinned relation view, work limits and truth evaluation

Commit subject `Separate pinned relation lookup and work accounting from exact evaluation`.

| Defining file under `query_execution/relations/` | Contents after the split |
|---|---|
| `mod.rs` | Module declarations for `view`, `work`, `evaluation` and `target_cache`, plus a short explanation of their responsibilities. Use defining-module imports for callers. |
| `view.rs` | `MergedRecordSlot`, `MergedRecord`, `MergedRelationView`, its worker fork, catalog/segment lookup, visible address iteration, source diagnostics, reference identity/backlink lookup and completeness state. Retain private lookup types and caches with their storage. |
| `work.rs` | `RelationWork`, its four query-wide traversal limits, `charge`, `check_query_work_budget` and the methods that check cancellation, liveness, bytes, visits and reads. Retain exact overflow, budget and streaming error behavior. The separate semijoin construction limits stay under the provider. |
| `evaluation.rs` | `EvaluationMode`, `TraverseConfig`, FILTER/FTS truth, membership and evidence entry points, merged and base hop visitors, quantifier reduction, source/target text selection and independent logical-occurrence traversal. Keep the borrowed-target test counter with the code that increments it. |
| `target_cache.rs` | The projected target prefetch moved in Commit 1. Keep its query lifetime, admission checks, pre-read resource charges and field-coverage checks. |

**Pinned lookup.** Preserve the collection, segment and local slot in every address, plus active membership in that publication. Worker forks retain the same pinned repository, catalog and segments, share the existing target-value cache, and keep the existing worker-local reference lookups and source-error logs. Use the view's `repository()` and `repository_root()` accessors where the old single file read fields directly. For the merged target cache, keep its current shard choice and the lock across the same lookup, miss, read and admission region. Where the split requires access, use a narrowly scoped sibling operation or restricted visibility. Preserve the existing guard release before invoking the terminal callback. Splitting lookup and insertion into separate lock acquisitions can decode a shared target twice.

**Work limits.** Keep budget checking before the bounded read, charging at the existing operation, and query-wide aggregation across scan chunks. Only expose sibling access to the methods, constants or execution-context reference actually needed. Move the private `relation_work_limits_return_explicit_errors` test into `work/tests.rs`. Its direct checks on private counters and charging methods belong there. The other 32 reviewed relation-query tests can remain under `evaluation/tests.rs` with explicit imports from `view` and `work`. This preserves test access without making private state public.

**Reference independence.** Binding and pinned record access can be shared. In reference mode, derive logical ID occurrences, positional selectors, ID resolution and reverse membership from visible source records as the existing path does. Keep the distinction between absent, null, empty, raw and unavailable records. Preserve the independent `logical_references` walk and its ordering. Allow the view to call the same logical-occurrence helper at restricted visibility where needed. Preserve the existing per-query reference lookup reuse and completeness flag. An optimized path can consult physical edges, stored columns, provider proofs and posting memberships. The reference path must still produce the correct answer when the tests deliberately perturb or remove those contributions.

**Planner and membership.** Keep the provider's offers, costs, domain checking and selection of `TargetFirst` versus `SourceRestricted`. Estimating or explaining an offer must preserve the existing zero target-read behavior. Keep candidate restrictions, proven matches and residual evaluation distinct through negation and quantifiers. Preserve separate occurrence positions for repeated IDs and separate segment identities for equal local slot numbers. Borrow or move the existing membership representation through the provider and runner. Preserve existing shared handles. Review any new bitmap clone, repeated target decode, copied logical value, allocation proportional to all targets or conversion to a flat slot set.

### Focused relation acceptance

Run these suites after the mechanical move and again after the split. A selector that executes zero tests does not verify a move. Counts below describe the reviewed Linux base. Reconcile any actual predecessor change by name.

| Suite or control | Expected evidence |
|---|---|
| `relation_oracle`, `relation_occurrence_oracle`, `non_json_relations` integration targets | 3, 1 and 1 tests respectively. Cover source-backed answers, duplicate occurrences and non-JSON reader dispatch. |
| Relocated unit suites | 33 former relation-query tests (32 evaluation + 1 work), 11 semijoin tests, 6 target-cache tests. Retain resource, mid-work cancellation, repeated local slot and independent strategy fixtures. |
| Three `base_reference_` controls | The prefix must execute `base_reference_sort_ignores_perturbed_stored_and_relation_contributions`, `base_reference_resolves_omitted_id_map_targets_and_materializes_their_bytes`, `base_reference_multi_target_backlinks_scan_each_relation_once`. |
| Separate segmented control | Run `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock` by its fully qualified name and see one pass. Also run the deliberately perturbed forward/backlink test and the old-pin retirement/catalog tests. |
| Incomplete backlink and sorting | `incomplete_backlink_discovery_stops_before_target_sort_cache_work` must retain incomplete count, exact source read/byte charges and zero sort-target reads. `reference_backlink_sort_streams_discovery_without_target_cache_budget` expects three sort-target reads. `related_sort_does_not_reuse_an_incomplete_filter_target_projection` exercises two reads and zero reads in its distinct scenarios. Keep the fixture-specific expectations. |
| Selected provider and work | Compare A2 and C for the same small requests. Record selected strategy where offered, candidate versus proof membership, exact ordered results and evidence, target reads, bytes, edge visits and target reuse. Retain the test that checks estimate-only explain opens/parses no related sources. Verify an old view again after a replacement is published. |

```sh
cargo test --locked -p objekte-search-core --test relation_oracle --test relation_occurrence_oracle --test non_json_relations
cargo test --locked -p objekte-search-core --lib query_access::relations::semijoin::tests::
cargo test --locked -p objekte-search-core --lib query_execution::relations::
cargo test --locked -p objekte-search-core --lib query_execution::relations::evaluation::tests::base_reference_
cargo test --locked -p objekte-search-core --lib query_execution::relations::evaluation::tests::segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock -- --exact
```

### Commit 3 · Finish crate-root ownership and caller documentation

Commit subject `Group cross-subsystem core tests and document canonical module paths`.

| Old file and Rust test prefix | New file and test prefix |
|---|---|
| `lib.test.rs` · `tests::` for the original root fixture test only | `tests/fixture_layout.rs` · `tests::fixture_layout::` |
| `foundation_invariants.test.rs` · `foundation_invariants::` | `tests/foundation_invariants.rs` · `tests::foundation_invariants::` |
| `folder.test.rs` · `folder_tests::` | `tests/folder.rs` · `tests::folder::` |
| `id.test.rs` · `id_tests::` | `tests/id.rs` · `tests::id::` |

Declare `#[cfg(test)] mod tests;` once in `lib.rs` and its four children in `tests/mod.rs`. Preserve crate-private unit-test access, fixture paths, original test names and registration counts (1, 1, 2, 2 on the reviewed base). Retain `workload.rs` and `workload/tests.rs`, the serialized workload format and `Query::from_workload`. List direct root Rust files before and after. On the reviewed tree `lib.rs` and `workload.rs` are the intended final pair. Account explicitly for any independently added file by owner.

Add a short crate-doc ownership table showing where a contributor registers a new reader, changes physical posting storage, changes a planner decision, or publishes a revised index. Keep public entry points distinct from private implementation modules. Update maintained Rust imports, Markdown snippets, doctests and benchmark or validation scripts that name old modules, test selectors or source files. Search for old paths in qualified, grouped and aliased forms. Historical reports describe their original revisions.

Build one old-to-new public import table for the **whole series** using the Assignment 1 mapping and this assignment's three relation types. Assignment 2's inherent JSON conversion methods retain their call syntax. Compare complete default and all-feature test inventories at B and C, including each ignored-only list and the integration targets. Compose the per-assignment test-name maps, require exactly one final match per baseline test and identify deliberately added coverage separately. Check that the feature-gated clock test still has its original discovery conditions. Compile any changed maintained Markdown example against the actual final public API.

### Final acceptance on the combined source

Run focused controls and `cargo check --locked --workspace --all-targets` for each source commit. On C, run the full suite once and save commands, SHA, exit status and actual counts. Run Clippy using the repository convention on the affected workspace and distinguish any reproducible baseline diagnostics. Include Assignment 2's identity and combined-parse controls and a pinned-revision control in the report.

```sh
cargo fmt --all -- --check
python3 scripts/format-rust-spacing.py --check
cargo check --locked --workspace --all-targets --all-features
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
env -u OBJEKTE_LARGE_COMPACT_RECORDS cargo test --release --locked -p objekte-search-core --lib large_source_free_compaction_cancels_and_retries_a_concurrent_commit -- --ignored --nocapture
```

The last command must execute one ignored release test with the default 262,144-record fixture. Keep the public caller and compile-fail privacy checks. Reproduce failures on the relevant starting revision under the same conditions and repair any regression before recording acceptance.

### Performance comparison · B against C

Use the existing release drivers and comparators. Reserve the full workload for this final pass. Resolve one canonical physical corpus, typically `/home/simon/source/objekte/examples/musenalm`, and pass that same absolute path to both binaries. File timestamps affect inventory fingerprints, generated identity and locators. Record corpus fingerprint and inventory before/after, toolchain, build flags, worker counts and machine conditions. The historical Assignment 7 timings and signatures document the method. Measure B and C afresh.

**Build and run.** Use clean tracked source at each exact revision and separate absolute Cargo target directories. Execute each binary from its own worktree, because the query driver records the execution checkout as well as the embedded build revision. In the shell below, set `source_sha` to the side's full SHA, `target_dir` to its own build directory, `corpus` to the common resolved path, and `side_dir` to a fresh absolute output directory for that side. Repeat for B and C with the same settings. Use fresh output directories for reruns so each measurements file contains exactly six construction samples.

```sh
set -eu
test "$(git rev-parse HEAD)" = "$source_sha"
OBJEKTE_BUILD_REVISION="$source_sha" CARGO_TARGET_DIR="$target_dir" \
  cargo build --locked --release -p objekte-search-core \
  --example bench_musenalm --example bench_build_musenalm
mkdir -p "$side_dir/queries" "$side_dir/build"
"$target_dir/release/examples/bench_musenalm" \
  --root "$corpus" --profiles ascii-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 --result-dir "$side_dir/queries"
for alphabet in ascii unicode; do
  for repetition in 1 2 3; do
    "$target_dir/release/examples/bench_build_musenalm" \
      --root "$corpus" --alphabet "$alphabet" --build-workers auto \
      --schema-manifest-path "$side_dir/build/schema-manifest.json" \
      >> "$side_dir/build/measurements.jsonl"
  done
done
```

Run the construction executable directly. On the reviewed base the Python build driver does not forward `--root`. Record the resolved build worker count and CPU availability for each side. If the machine gives different worker counts, repeat the affected comparison with one common explicit worker count.

**Compare.** Run the following from C after setting `baseline_out`, `candidate_out`, `baseline_tree`, `candidate_tree`, `baseline_sha`, `candidate_sha`, `baseline_target`, `candidate_target` and `result_dir` to their recorded paths or SHAs. Keep both raw sides and the two comparison outputs together.

```sh
python3 bench/compare_ordered_queries.py \
  --baseline "$baseline_out/queries" --current "$candidate_out/queries" \
  --baseline-workload "$baseline_tree/crates/core/benches/musenalm_queries.json" \
  --current-workload "$candidate_tree/crates/core/benches/musenalm_queries.json" \
  --output "$result_dir/query-comparison.json"
python3 bench/compare_build_samples.py \
  --baseline "$baseline_out/build" --current "$candidate_out/build" \
  --baseline-revision "$baseline_sha" --current-revision "$candidate_sha" \
  --baseline-binary "$baseline_target/release/examples/bench_build_musenalm" \
  --current-binary "$candidate_target/release/examples/bench_build_musenalm" \
  --root "$corpus" --output "$result_dir/build-comparison.json"
```

| Measure | Acceptance and reporting |
|---|---|
| Ordered query results | Check the expected 150 manifest case IDs and all three repetition keys per case on both sides. Require 150/150 equal ordered digests, 450/450 paired samples, successful statuses without errors, equal counts, returned page sizes and all recorded work counters. Require equal schema, resolved queries and workload bytes, matching corpus fingerprints and empty compatibility failures. Keep the page order and duplicate occurrences in the comparison. |
| Query time | For each case take the median of its three `total_ns` samples. Provide the paired medians and differences, then summarize the largest slowdowns in milliseconds and percent, with case IDs. Repeat only the affected cases using `--cases` if a material difference needs confirmation. Preserve the original measurements. |
| Construction time and memory | Require three ASCII and three Unicode samples per side, equal schema-manifest digests across sides, matching index signatures per alphabet, and equal record/parse/failure counts. Report median and range of wall time and peak RSS from raw samples, plus B→C differences. Preserve binary hashes, revision stamps and worker counts. |
| Diagnosing a difference | For result or work changes, report the first mismatched query, field and ordered hit position. For repeatable time or RSS regressions, narrow to the responsible series commit and operation using the affected case or build. Correct the responsible stage and revalidate affected gates at the new C. Report a remaining regression explicitly. |

Append the report to `docs/ai/done/objekte_core_assignment_3_relations_and_final_integration.md`. Include B, A1, A2, the stage commits, C, root inventories, public imports, test mappings and counts, targeted oracle/read evidence, and raw/comparison paths. Put the measured evidence on the integration branch following the existing benchmark layout. Commit the report and evidence as `Record core refactoring integration verification`, push, then report A3 and the exact tested C to the owner. The completion condition is one reviewed series ready for the owner's merge.

[Pinned relation implementation](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/relation_query.rs) · [Relation provider](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/query_access/relations.rs) · [Verified same-corpus procedure](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/docs/ai/done/objekte_core_assignment_7_verification_and_documentation_repair.md)

## Implementation report

- Task identifier: `core-refactoring-completion` / Assignment 3
- Completion date: 2026-11-19
- Series baseline B: `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`
- Assignment 1 delivery A1: `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee`
- Reviewed Assignment 2 delivery A2: `e6abf1a00eada9731dedb7466aaebb7b15771527`
- Assignment 2 pread64-repair entry: `9ae617b5df3f0a082aa923195a28019d5c36815d`
- Tested final source C: `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

Git ancestry verifies B → A1 → A2 → the A2 repair → C.

### Delivered structure

The three Assignment 3 source commits are:

1. `9c62fedec51ce846612fa1b58cb7ea1a5bb0fd26` — `Assign relation declarations, provider operations and target caching to their owners`
2. `0b36bc256fee1ada7c7efa0fd0a931f5af4478a1` — `Separate pinned relation lookup and work accounting from exact evaluation`
3. `acac94f486ee0f9d35eb5ba834d26cdc01eb1c5e` — `Group cross-subsystem core tests and document canonical module paths`

Relation declarations, `Schema` registration and relation validation now live
in `schema::relations`. Provider strategies and construction limits live in
`query_access::relations::semijoin`. The physical relation edge storage remains
in `index::relation`; query binding and runner orchestration remain with their
existing owners.

The former relation query is split into four crate-private owners:

- `query_execution::relations::view` holds pinned merged-record lookup,
  reference identity/backlinks and source diagnostics.
- `query_execution::relations::work` holds query-wide edge, visit, byte and
  record-read limits and their exact typed error behavior.
- `query_execution::relations::evaluation` holds truth, membership, evidence,
  hop traversal and quantifier evaluation.
- `query_execution::relations::target_cache` holds projected target
  preparation, admission, reuse counting and resource charges.

Worker forks still share the pinned repository, catalog, segments and target
cache. The cache lock spans lookup, miss, read and admission; it is not split
into a lookup followed by a separately locked insertion. Reference mode
continues to derive logical occurrences and reverse membership from visible
source records. The independent logical-reference walk and completeness state
remain intact.

The four root cross-subsystem suites are now under `tests/`. The final direct
Rust files under `crates/core/src/` are only `lib.rs` and `workload.rs`.

### Whole-series public import mapping

| Old path | Final path |
|---|---|
| `path_repr::query_path` | `model::path_representation::query_path` |
| `discovery::*` | `inventory::discovery::*` |
| `diagnostics::{milliseconds_3, signed_milliseconds_3}` | `measurement::format::{milliseconds_3, signed_milliseconds_3}` |
| `completion::*` | `query::completion::*` |
| `ReturnMode`, `SortField`, `SortExpression`, `SortReducer` | `query::options` |
| `CompositeSortPart` | `index::sorting::CompositeSortPart` |
| `BigramAlphabet` | `index::bigram::BigramAlphabet` |
| `streaming::*` | `query_execution::streaming::*` |
| `posting_values::*` | `repository::posting_values::*` |
| `relation_inspection::*` | `repository::relations::*` |
| `relation::RelationCardinality` | `schema::relations::RelationCardinality` |
| `relation::RelationDefinition` | `schema::relations::RelationDefinition` |
| `relation::RelationId` | `schema::relations::RelationId` |

Assignment 2's `Value::from_json`, `Number::from_json` and related inherent JSON
conversion methods retain their call syntax. Public relation calls retain
`Schema::relations` and `Schema::register_relation`. The moved relation query,
semijoin and target-cache modules are private implementation owners and have no
public forwarding aliases.

### Relation strategy and reference evidence

The extended-schema A2/C probe used the same Musenalm corpus, four query
workers and zero verified-cache bytes. Across Q102, Q122 and Q088, all nine
paired samples had equal statuses, errors and complete `WorkCounters`; all
three ordered digests and the normalized plans matched. The archived schema
manifests are byte-identical.

| Case | Request | Ordered result | Selected plan | Candidate/proof records | A2 median | C median |
|---|---|---|---:|---|---:|---:|
| Q102 | `related_entries.entry ?= "00fx8ln651na0e3"` | 0 matches | exact posting, 0 output from 1,202 input records | `0/0` at every observation | 42.416 µs | 46.116 µs |
| Q122 | `related_entries:length > 0` | 67 matches, 50 returned | exact posting, 67 output from 1,202 input records | `67/67` at every observation | 59.841 µs | 58.285 µs |
| Q088 | `agents.agent ?= "lngx6eyfw5qrzas"` | 5 matches | exact posting, 5 output from 4,928 input records | `5/5` at every observation | 51.683 µs | 58.766 µs |

These direct typed-ID requests resolve to exact posting plans without target
reads. The relation traversal path itself is independently covered by the
focused tests:

- `both_physical_strategies_keep_independent_witnesses_and_reuse_targets`
  obtains the same 40-record ordered result from `TargetFirst` and
  `SourceRestricted`. `TargetFirst` reads at most one target; the restricted
  path reads at most 25 but returns the same `S00` through `S39` order.
- The unselective selected-field FTS selects `relation target-first`, with at
  most one target read, one target candidate, one reverse posting and 40
  reverse members, while scanning zero source records. The selective local
  filter plus relation FTS selects `relation source-restricted`, returns
  `S00`, visits one edge and scans zero records.
- The multi-hop request `entry.related_entry.title ?= 'Festival'` retains the
  ordered `S00` through `S58` result and selects one of the two relation
  strategies.
- Target-first and source-restricted candidate sets agree. Negation, universal
  comparison, fallback OR, snapshot restore and estimate-only explain retain
  their independent reference behavior.

The target-cache fixtures reuse at least 79 of 80 verified targets with at most
two target opens/reads and positive relation bytes. The incompletely projected
target fixture starts again instead of reusing stale truth. Reverse backlink
probes preserve unpositioned versus positional identity order.

The incomplete-backlink control reads eight 2 MiB linked sources
(`relation_bytes_read = 16,777,216`), opens nine files, parses nine records,
charges 16,777,243 public bytes, performs zero sort-target reads and marks the
count incomplete. Reference backlink sorting returns ordered `A`, `B` with
three sort-target reads. Related sorting returns `m8`, `m1`; its distinct
projection cases perform two and zero target reads, with at least two verified
target reuses in the second case.

### Root and test inventories

B had 43 direct Rust files under `crates/core/src/`:

`completion.rs`, `decimal.rs`, `decimal.test.rs`, `detailed_timer.rs`,
`diagnostics.rs`, `discovery.rs`, `discovery.test.rs`, `execution_options.rs`,
`execution_options.test.rs`, `folder.test.rs`,
`foundation_invariants.test.rs`, `id.test.rs`, `inference.rs`,
`inference.test.rs`, `inference_build.test.rs`, `inventory.rs`,
`inventory.test.rs`, `lib.rs`, `lib.test.rs`, `measurement.rs`, `model.rs`,
`model.test.rs`, `path_repr.rs`, `path_repr.test.rs`, `posting_values.rs`,
`posting_values.test.rs`, `record_preparation.rs`,
`record_preparation.test.rs`, `relation.rs`, `relation_inspection.rs`,
`relation_inspection.test.rs`, `relation_query.rs`, `relation_query.test.rs`,
`relation_semijoin.rs`, `relation_semijoin.test.rs`, `relation_target_cache.rs`,
`relation_target_cache.test.rs`, `streaming.rs`, `streaming.test.rs`,
`structural.rs`, `structural.test.rs`, `workload.rs`, and `workload.test.rs`.

C has only `lib.rs` and `workload.rs`.

Assignment 1 test namespace moves:

| Old namespace | Final namespace |
|---|---|
| `decimal::tests` | `model::decimal::tests` |
| `path_repr::tests` | `model::path_representation::tests` |
| `structural::tests` | `model::structure::tests` |
| `discovery::tests` | `inventory::discovery::tests` |
| `detailed_timer::tests` | `measurement::timer::tests` |
| `diagnostics::tests` | `measurement::format::tests` |
| `completion::tests` | `query::completion::tests` |
| `execution_options::tests` | `query::options::tests` |
| `streaming::tests` | `query_execution::streaming::tests` |
| `posting_values::tests` | `repository::posting_values::tests` |
| `relation_inspection::tests` | `repository::relations::tests` |
| `inference::build::allocation_tests` | `construction::inference::tests` |

Assignment 2 moved
`record_preparation::tests::build_rejects_non_finite_numbers_before_indexing_nested_values`
to `construction::file_preparation::tests` and the other five
`record_preparation::tests` to `source::preparation::tests`. Its six added
identity tests are listed below.

Assignment 3 test namespace moves:

| Old test namespace | Final namespace | Tests |
|---|---|---:|
| `lib.test.rs` root fixture | `tests::fixture_layout::` | 1 |
| `foundation_invariants::` | `tests::foundation_invariants::` | 1 |
| `folder_tests::` | `tests::folder::` | 2 baseline + 1 A2 matrix test |
| `id_tests::` | `tests::id::` | 2 |
| `relation_query::tests::` | `query_execution::relations::evaluation::tests::` | 32 |
| `relation_query::tests::relation_work_limits_return_explicit_errors` | `query_execution::relations::work::tests::` | 1 |
| `relation_semijoin::tests::` | `query_access::relations::semijoin::tests::` | 11 |
| `relation_target_cache::tests::` | `query_execution::relations::target_cache::tests::` | 6 |

`inventory/reconciliation.json` applies the complete A1, A2 and A3 mappings.
For default tests, B has 928 entries and C has 934; 125 baseline entries map to
their final namespaces. For all-feature tests, B has 930 and C has 936; 124
map. The difference is the default-only ordinary-clock test.

The six final added tests are:

- `folder_identity_defaults_and_relative_path_eligibility_follow_format`
- `jsonl_identity_checks_exact_ranges_classes_and_fallback_identity`
- `registered_json_uses_generated_ids_with_builtin_scalar_verification`
- `relative_path_errors_name_one_valid_file_and_exact_range`
- `relative_path_identity_checks_whole_file_ranges_and_fallback_ids`
- `registered_readers_do_not_enable_relative_path_identity`

The compile-fail doctest moved from line 40 to line 42. Every mapped B test has
exactly one C match, apart from those explicitly identified additions. Both
profiles retain 18 ignored entries; the ignored name sets are identical. The
ordinary-clock test is present in the default inventory and absent with
`detailed-profiling`, as before.

Core-library inventories grew only because of the six A2 identity tests:
621/622 at B and 627/628 at C for default/all-feature. The A3 relation split
changes paths but no test count.

### Focused controls

| Control | A2 | C |
|---|---:|---:|
| Former relation-query tests | 33 | 32 evaluation + 1 work |
| Semijoin provider tests | 11 | 11 |
| Target-cache tests | 6 | 6 |
| `relation_oracle`, occurrence and non-JSON relations | 3, 1, 1 | 3, 1, 1 |
| Three `base_reference_` controls | 3 | 3 |
| Segmented retired/new-row reference parity | 1 | 1 |
| Incomplete backlink, reference backlink sort and related sort | Passed | Passed |

Assignment 2's JSON/JSONL/path/generated-ID matrix,
`decoded_scalar_fts_combines_source_text_with_required_values`,
`borrowed_source_verification_matches_reference_for_outputs_and_duplicates`,
`exact_membership_materializes_only_page_payload`,
`live_configuration_hash_matches_the_pre_restore_fixture`, registered-reader
restore and combined parse checks remain green at C.

### Full validation

| Command | Outcome at C |
|---|---|
| `cargo check --locked --workspace --all-targets --all-features` | Passed |
| `cargo test --locked --workspace` | Passed: 916 passed, 18 ignored across all targets |
| `cargo test --locked -p objekte-search-core --all-features` | Passed: 642 passed, 16 ignored |
| `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps` | Passed |
| `cargo clippy --locked --workspace --all-targets --all-features` | Passed with inherited warnings |
| Ignored release compaction control | 1 passed with the default 262,144-record fixture |
| `cargo fmt --all -- --check` | Fails only on the inherited `model/json.rs` blank-line disagreement |
| `python3 scripts/format-rust-spacing.py --check` | Fails the inherited workspace-wide spacing baseline |

The B, A2 and C spacing checks report 199, 199 and 201 files. The two
additional C entries are the extra files produced by splitting the one checked
`relation_query.rs` into `evaluation.rs`, `view.rs` and `work.rs`. No unrelated
file was reformatted.

Plain rustfmt passes at B, requests one blank line around the two `impl` blocks
in `model/json.rs` at A2/C, while the repository spacing convention requires
two. This is the known mutually incompatible formatter baseline recorded by
Assignment 2. It is retained without changing the source used for C.

Clippy reports 194 library and 220 library-test warnings at B, and 195 and 221
at C. The additional diagnostic is `clippy::type_complexity` on the narrowly
scoped shared target-cache guard returned by
`query_execution::relations::view::lock_target_value_cache`; all other warning
classes correspond to moved baseline locations. Clippy exits successfully.

The ignored release test reported:

```text
large compaction build records=262144 elapsed=257.505015ms
large compaction records=262144 elapsed=261.054679ms attempts=2 before=262146 after=262145 retired=1
1 passed; 626 filtered out
```

No maintained Markdown Rust example changed in the series. The crate-doc
ownership table and public callers were compiled by workspace documentation
and doctest gates; the public compile-fail privacy check still passes.

### B/C Musenalm comparison

The canonical corpus was
`/home/simon/source/objekte/examples/musenalm`. Its 148,501-file inventory
totaling 326,105,444 bytes had the same SHA-256 inventory digest before and
after:
`3f4fb734a59049735dba22879ef07abc44a00f5fa3b1b8c2f90fc9d536db2055`.
The indexed source registration contains 148,486 records with the common
fingerprint
`9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`.

Measurements used Rust 1.99.1, a 13th Gen Intel Core i9-13900K with 32 logical
CPUs, four query workers, zero verified-cache bytes and three repetitions and
build samples per alphabet. B and C used distinct absolute Cargo target
directories and binary-build revision stamps.

Query parity:

| Measure | Result |
|---|---:|
| Ordered result digests | 150/150 |
| Paired sample keys | 450/450 |
| Empty/non-empty cases | 11/139 |
| Status or error differences | 0 |
| Work-counter differences | 0 |
| Schema/resolved-query/workload-byte differences | 0 |
| Compatibility failures | 0 |

Raw query evidence is under
`musenalm/common-root/{baseline,candidate}/queries`; the repository comparison
is `musenalm/comparison/query-comparison.json`.

The per-case median analysis is in
`musenalm/comparison/performance-analysis.json`. Of 150 cases, 53 were slower
and 97 faster; the median case difference was −0.002302 ms. The largest
absolute slowdowns were:

| Case | B median | C median | Difference | Percent |
|---|---:|---:|---:|---:|
| Q136 | 3.018297 ms | 3.429681 ms | +0.411384 ms | +13.63% |
| Q012 | 9.483678 ms | 9.866285 ms | +0.382607 ms | +4.03% |
| Q001 | 12.336884 ms | 12.719077 ms | +0.382193 ms | +3.10% |
| Q055 | 4.425102 ms | 4.585254 ms | +0.160152 ms | +3.62% |
| Q132 | 0.811003 ms | 0.902591 ms | +0.091588 ms | +11.29% |

All largest regressions are sub-millisecond absolute differences. There was no
repeatable material regression requiring a narrowed rerun, and ordered results
and work are identical.

Construction parity:

| Alphabet | B wall median (range) | C wall median (range) | Difference | B peak RSS median | C peak RSS median |
|---|---:|---:|---:|---:|---:|
| ASCII | 626.169 ms (609.370–700.969) | 636.465 ms (613.568–674.024) | +10.296 ms / +1.64% | 628.8 MiB | 623.6 MiB |
| Unicode | 716.114 ms (691.438–723.372) | 714.433 ms (682.702–717.895) | −1.681 ms / −0.23% | 647.3 MiB | 631.6 MiB |

Each side has three samples per alphabet, 148,486 records and parses, and zero
failed parses. ASCII signatures
`4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f`
and Unicode signatures
`5c235a33b6cb1a18c1a1f4d2b1d60fa1eae912ce2b4ff868857df95028d83868`
match across sides. The schema-manifest digest
`38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`
also matches. Both sides resolved the automatic build worker count to 32.

Raw construction samples and manifests are under
`musenalm/common-root/{baseline,candidate}/build`; the comparison is
`musenalm/comparison/build-comparison.json`. Binary hashes and revision stamps
are retained by the comparators and run manifests.

### Evidence index and cleanup

The raw and compact evidence, reproduction scripts, test logs and artifact
index are under
[`assignment-3-relations-final-integration`](../generated/core-refactoring-completion/assignment-3-relations-final-integration/README.md).

The integration worktree and branch are retained for owner review. The
detached B and A2 comparison worktrees, task memory and obsolete probe are
removed after this document and evidence are pushed.
