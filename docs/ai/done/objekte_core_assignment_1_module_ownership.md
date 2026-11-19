# Objekte core completion · Assignment 1 of 3
## Module ownership and inference construction

**Reviewed base** `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`, the Assignment 7 merge on `master`.  
**Integration branch** `integration/core-refactoring-completion`  
**Integration worktree** `/home/simon/source/objekte-integration-core-refactoring-completion`  
**Review point** Three implementation commits followed by one completion-report commit. Deliver the fourth commit and stop for owner review.

### Mandate

Give the shared model, file inventory, measurement, query options, repository inspection and inference construction their defining Rust module paths. Move the associated tests and update every maintained caller in the same commit as its definition. All paths in the move tables are relative to `crates/core/src/`.

This document is the operative specification for the first three stages of the completion roadmap. Assignment 2 owns record preparation, JSON conversion, reader capability and identity policy. Assignment 3 owns relation binding/evaluation changes, semijoins, relation caches and final integration benchmarking. This assignment moves the **public relation inspection API**. Its traversal and paging bodies carry over verbatim. In later-assignment files, change imports and qualified references only where this assignment moves a dependency.

### Baseline and worktree

1. Read `AGENTS.md`, `docs/ai/README.md` and `docs/development/code-guidelines.md`. Treat the three assignments as review points on one integration branch. One implementing agent owns these sequential commits. Leave the original checkout on `master`. Push after each commit.
2. Fetch `origin/master` and the integration ref. For a new branch, start at fetched `origin/master` and record its full SHA as **B**. Verify that B contains the reviewed base. If the integration branch already exists, reuse its sibling worktree and published history. Resume a verified completed step rather than replaying it. Record the actual pre-assignment SHA. If a newer commit changes a listed definition or public contract incompatibly with these directions, report the exact symbol and diff to the owner before changing that contract.
3. At B, capture the Linux default and all-feature core unit-test listings, plus the ignored-only listings for each profile. Use `cargo test --locked -p objekte-search-core --lib -- --list`. Add `--all-features` before the harness separator for the second profile and `--ignored` after it for the ignored-only variants. Record fully qualified names. Capture the output of the small inference fixture specified below while still at B. Keep temporary evidence in the task's working area.
4. Apply the public path changes listed below. Remove retired declarations from `lib.rs` in their owning commit. Each item has one defining path. Migrate direct, grouped, aliased and absolute imports, public signatures, doctests, Markdown examples, Serde path strings, and maintained scripts containing a source path or test selector. Inventory with `rg` across the active workspace. Historical specs, archived code and recorded benchmarks describe their original revisions.

### Commit 1 · Place model, inventory and measurement code with its owner

**Commit subject:** `Group core model, inventory and measurement modules`

| Existing file | New file | Declaration in the parent |
|---|---|---|
| `model.rs` | `model/mod.rs` | Keep `pub mod model` and its existing `pub mod text`. |
| `decimal.rs` | `model/decimal.rs` | `pub(crate) mod decimal` |
| `path_repr.rs` | `model/path_representation.rs` | `pub mod path_representation` |
| `structural.rs` | `model/structure.rs` | `pub(crate) mod structure` |
| `inventory.rs` | `inventory/mod.rs` | Keep `pub mod inventory`. |
| `discovery.rs` | `inventory/discovery.rs` | `pub mod discovery` |
| `measurement.rs` | `measurement/mod.rs` | Keep `pub mod measurement`. |
| `detailed_timer.rs` | `measurement/timer.rs` | `pub(crate) mod timer` |
| `diagnostics.rs` | `measurement/format.rs` | `pub mod format` |

The public import moves are `path_repr::query_path → model::path_representation::query_path`, all existing public `discovery` items → `inventory::discovery`, and both signed and unsigned millisecond formatters → `measurement::format`. Keep `model::Value`, `model::Number`, the record IDs, inventory types and `measurement::WorkCounters` at their current public paths. Keep `append_shifted_bitmap` in inventory. Keep the JSON conversions inside the moved model file for Assignment 2. Keep construction discovery and lifecycle catalog under their present owners.

Move companion suites according to this exact mapping. Use a normal `#[cfg(test)] mod tests;` in each owner. For the timer, copy the existing compound feature condition verbatim.

| Existing tests | New file |
|---|---|
| `model.test.rs` | `model/tests.rs` |
| `decimal.test.rs` | `model/decimal/tests.rs` |
| `path_repr.test.rs` | `model/path_representation/tests.rs` |
| `structural.test.rs` | `model/structure/tests.rs` |
| `inventory.test.rs` | `inventory/tests.rs` |
| `discovery.test.rs` | `inventory/discovery/tests.rs` |
| Inline suites in `detailed_timer.rs` and `diagnostics.rs` | `measurement/timer/tests.rs` and `measurement/format/tests.rs`, respectively |

**Review.** Confirm unchanged exact decimal comparisons, native path escaping, file ordering, symlink selection and timing calls. The ordinary clock test must still prove zero-sized timer state and zero elapsed time with detailed profiling disabled. On Linux the existing path fixture must still produce `a/%FF%2520%5C.json`. Run the relocated suites and the `discovery_exclusions` integration target.

### Commit 2 · Assign public query options and inspection operations

**Commit subject:** `Place query options and repository inspection with their defining modules`

| Existing source or symbol | New defining module | Required placement |
|---|---|---|
| `completion.rs` | `query::completion` | Entire file to `query/completion.rs`. Public caret parser, insertion function and types move together. Move its inline tests to `query/completion/tests.rs`. |
| `ReturnMode`, `SortField`, `SortExpression`, `SortReducer` and `impl SortField` | `query::options` | Put these definitions in new `query/options.rs`. Existing `query::QueryOptions` stays where it is. |
| `CompositeSortPart`, `simple_composite_part` | `index::sorting` | Define directly in existing `index/sorting/mod.rs`. The type stays public and the helper stays crate-private. Preserve the existing simple-sort eligibility test in the helper. |
| `BigramAlphabet` | `index::bigram` | Define directly in existing `index/bigram/mod.rs`. Preserve variant order, derives and lower-case Serde encoding. Remove the old root `execution_options` module after all six types and the helper have moved. |
| `streaming.rs` | `query_execution::streaming` | Entire file to `query_execution/streaming.rs`. Move all public types and internal helpers together. |
| `posting_values.rs` | `repository::posting_values` | Entire file to `repository/posting_values.rs`, including exact selection and cursors. Preserve the methods' receivers on the repository or view. |
| `relation_inspection.rs` | `repository::relations` | Entire file to `repository/relations.rs`, including graph, outgoing and backlink pages. Keep the existing private `repository::inspection` module and physical `index::relation` separate. |

Declare the new public modules in their defining parents. Move the last three companion suites to `query_execution/streaming/tests.rs`, `repository/posting_values/tests.rs` and `repository/relations/tests.rs`. Move the single test in `execution_options.test.rs` intact to `query/options/tests.rs`. Import the alphabet from `index::bigram`, use the query types from `query::options`, and retain the assertions for `"logical"` and `"unicode"`. Keep its original function name for the test inventory. Keep `workload.rs` and its public serialized format. Move `workload.test.rs` to `workload/tests.rs` and use normal test-module discovery.

**Review.** All workspace callers must compile from their new paths, including the desktop, CLI and Cargo examples. Preserve callback lifetimes, cancellation, candidate-membership ownership, cursor revision checks, liveness filtering, counts and page order. In the production diff, these moves require no new allocation, bitmap clone, source read, timer call or public forwarding alias. Run the relocated suites, the workload manifest round trips, the `inline_sort` integration target and the existing configuration-identity golden test.

### Commit 3 · Make construction the actual owner of inference building

**Commit subject:** `Declare inference construction under the construction module`

1. Move `inference.rs` to `inference/mod.rs`. Keep public policy, configuration, reports, statistics, published `InferenceState` and all their methods in `crate::inference`. Keep `INFERENCE_POLICY_VERSION` at its existing value, 5 on the reviewed base.
2. In `construction/mod.rs`, declare `pub(crate) mod inference;`. The existing file `construction/inference.rs` remains in place. Remove the `#[path = "construction/inference.rs"]` declaration, `mod build` and builder/observation re-exports from the old owner.
3. Move the exact definition of `ObservedStep` and the body of `typed_path` from the old root file to `construction/inference.rs`. Keep `ObservedStep` crate-private so the existing cross-module test can check that the field name `"0"` differs from array index 0. Keep `typed_path` private to construction inference. Retain the typed key, `Arc<str>` interning and both variants. In the construction file replace the old parent imports for policy and report types with explicit `crate::inference::{…}` imports.
4. Define `InferenceBuilder`, `InferenceObservation`, `candidate_config` and `aggregate_segment_inference` only at `crate::construction::inference`. Update all callers. In particular, batch and segment admission import the builder there, repository-state assembly calls aggregation there, and `index/segment/mod.rs` imports the observation there while retaining `InferenceState` from `crate::inference`. The segment keeps its existing observation representation. Do not rebuild it from published dictionaries.
5. Move `inference.test.rs` to `inference/tests.rs`. Import `InferenceBuilder` and `ObservedStep` from construction in those tests. Move `inference_build.test.rs` to `construction/inference/tests.rs`. Replace the old cross-directory `mod allocation_tests` declaration with `#[cfg(test)] mod tests;`. The eight allocation/merge tests therefore map from `inference::build::allocation_tests::` to `construction::inference::tests::`. Keep their private access through `super` and all eight bodies.

**Review.** Preserve observation order, candidate admission, shifted slot domains, budget charging, worker limits, buffer reuse, dictionary coverage and report ordering. Run the 38 existing `inference::tests::` cases and all eight construction cases on the reviewed base. The cases for global thresholds, segment order, budget exhaustion and reuse of path traversal storage exercise different failure modes. A green suite with an empty selector fails acceptance.

### Fixture comparison

Use the existing `bench_inference_fixture` example on B and on the final source commit. Create the following input once outside the worktrees. Retain the same absolute `fixture_root` for both revisions.

```sh
fixture_root=$(mktemp -d)
python3 - "$fixture_root" <<'PYTHON'
import json
import sys
from pathlib import Path

data = Path(sys.argv[1]) / "data"
data.mkdir()
with (data / "records.jsonl").open("w", encoding="utf-8") as output:
    for i in range(32):
        record = {"manual": "m", "blocked": "x", "group": ["A", "B"][i % 2],
                  "enabled": i % 2 == 0, "sequence": i}
        print(json.dumps(record), file=output)
PYTHON
```

At B set `revision_label=baseline`. At the final source commit set `revision_label=candidate`. Execute from the corresponding checkout.

```sh
for mode in disabled report auto; do
    cargo run --quiet --locked -p objekte-search-core --example bench_inference_fixture -- \
      --root "$fixture_root" --inference "$mode" > "$fixture_root/$revision_label-$mode.json" || exit 1
done
```

For each mode, compare these JSON keys with B using structural equality: `mode`, `root`, `files`, `records`, `build_parses`, `inference_report` and `indexes`. Preserve the order of arrays. Require 32 records in every run and at least one inferred index in the baseline and candidate auto runs. Time, RSS and scheduling telemetry are outside this equality test. Report the three comparisons. This gives a direct check of built indexes and coverage without running the 150-case Musenalm suite.

[Existing fixture driver and output fields](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/examples/bench_inference_fixture.rs) · [Original builder declaration](https://github.com/Simon-Martens/objekte/blob/0f1d5e5ffa566a911f926ffe564e49c0c69ded77/crates/core/src/inference.rs#L456)

### Integration acceptance

After each implementation commit's changes, run its focused tests and `cargo check --locked --workspace --all-targets` before committing. On the final combined source, run:

```sh
cargo fmt --all -- --check
python3 scripts/format-rust-spacing.py --check
cargo check --locked --workspace --all-targets --all-features
cargo test --locked -p objekte-search-core --lib
cargo test --locked -p objekte-search-core --lib --all-features
cargo test --locked -p objekte-search-core --test discovery_exclusions --test inline_sort
cargo test --locked -p objekte-search-core --doc
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

On the reviewed Linux base, check these execution counts against the test output. If B is newer, use its recorded inventory and explain any upstream difference.

| Suite or control | Expected coverage |
|---|---|
| Caret completion, streaming, posting values, relation inspection | 9, 24, 6 and 3 unit tests respectively |
| Published inference and construction observation | 38 and 8 unit tests respectively |
| Public discovery and sort targets | 7 in `discovery_exclusions`, 5 in `inline_sort` |
| Configuration hash | `live_configuration_hash_matches_the_pre_restore_fixture` executes and passes |
| Feature-gated clock | `ordinary_build_has_no_detailed_clock_or_timer_state` executes in the default build and is absent with `detailed-profiling` |

Compare the **whole** baseline and final unit-test inventories separately for default, all features and their ignored subsets. Apply precisely the module-prefix moves in this spec. Each old test maps to exactly one final test with its original ignored status. Compile any maintained Markdown examples whose imports changed. Include the public old-to-new import table in the report. For a failing check, reproduce it on B with the same command and environment. Correct any assignment regression on the integration branch. Record a reproducible baseline failure separately, with both outputs. An incompatible public contract change requires owner review.

Keep the four root cross-subsystem suites (`lib.test.rs`, `foundation_invariants.test.rs`, `folder.test.rs`, `id.test.rs`) registered and running. Update their affected imports in this assignment. Their physical relocation is outside this review point. The number of Rust files directly under `src/` is descriptive and carries no pass/fail threshold.

### Handoff

Append the completion report to `docs/ai/done/objekte_core_assignment_1_module_ownership.md` according to the repository workflow. Record B, the three implementation SHAs, commands and executed counts, the fixture verdict per mode, the API/test mappings, and any unresolved difference. Name the report commit `Record core module ownership assignment results`. Report the pushed branch and full delivery SHA. The owner reviews Assignment 1 before the series proceeds to source preparation or relation evaluation. The full Musenalm build/query comparison belongs to final series integration.

## Implementation report

- Task identifier: `core-refactoring-completion` / Assignment 1
- Completion date: 2026-11-19
- Base commit B: `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

### Delivered changes

The reviewed base already contained B, so the integration branch started directly at B. The remote integration ref did not exist before this task.

The three implementation commits are:

1. `88a8575b8a` — `Group core model, inventory and measurement modules`
2. `7a48f15a7c` — `Place query options and repository inspection with their defining modules`
3. `587835f5a0` — `Declare inference construction under the construction module`

The changes place model helpers under `model`, discovery under `inventory`, timers and formatting under `measurement`, query option types under `query::options`, sort keys under `index::sorting`, the alphabet under `index::bigram`, streaming under `query_execution`, and posting/relation inspection under `repository`. Inference policy, configuration, reports and state remain at `crate::inference`; builders, observations, private typed paths, candidate configuration and segment aggregation now live only at `crate::construction::inference`.

Companion tests moved with their owners. Callers were migrated throughout the core, CLI, desktop, tests and examples. No public forwarding aliases were retained for retired paths.

### Public import mapping

| Old path | New path |
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

`InferenceBuilder`, `InferenceObservation`, `candidate_config` and `aggregate_segment_inference` are crate-private construction APIs. `ObservedStep` and `typed_path` moved from the published inference owner to construction. The existing public inference API remains under `crate::inference`.

### Test inventory mapping

| Old namespace | New namespace |
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

`model::tests`, `inventory::tests`, `workload::tests` and `inference::tests` retain their fully qualified test names.

### Validation

Each implementation commit passed `cargo check --locked --workspace --all-targets` plus its focused tests before commit. Focused results were:

- Commit 1: model subtree 14 passed, inventory subtree 15 passed, format 1 passed, ordinary clock 1 passed, `discovery_exclusions` 7 passed.
- Commit 2: completion 9, options 1, streaming 24, posting values 6, relations 3, workload 2, `inline_sort` 5, and the configuration identity golden test 1.
- Commit 3: published `inference::tests` 38 passed and `construction::inference::tests` 8 passed. The construction test count was checked independently from the published count.

Final commands and outcomes:

| Command | Outcome |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `python3 scripts/format-rust-spacing.py --check` | Failed at B and at the final source with the same pre-existing file set |
| `cargo check --locked --workspace --all-targets --all-features` | Passed |
| `cargo test --locked -p objekte-search-core --lib` | 605 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --lib --all-features` | 606 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --test discovery_exclusions --test inline_sort` | 7 and 5 passed |
| `cargo test --locked -p objekte-search-core --doc` | 1 passed |
| `RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps` | Passed |
| `cargo clippy --locked --workspace --all-targets --all-features` | Passed |

The spacing check is a reproducible baseline failure. At B the command exits 1 after printing `Rust blank-line convention is not satisfied` for 200 files. At the final source it exits 1 for the same maintained file set after applying the specified module path moves, totaling 199 current files. The failure is therefore not introduced by this assignment. Reformatting the entire workspace would create unrelated changes beyond this review point.

The ordinary clock test appears in the default inventory and is absent from the all-feature inventory, as required. The configuration hashes test appears in both inventories and passed.

### Fixture comparison

The fixture root was `/tmp/objekte-core-refactoring-fixture.ikib1a` for both revisions. Every mode produced 32 records. The compared keys were `mode`, `root`, `files`, `records`, `build_parses`, `inference_report` and `indexes`, with array order retained.

| Mode | Structural equality | Built inferred fields |
|---|---|---|
| `disabled` | Equal | None |
| `report` | Equal | None |
| `auto` | Equal | `enabled`, `group`, `sequence` |

The baseline example serialized `indexes` as `null` in `auto` even though the report marked three fields `Built`; a fresh-root probe reproduced that base behavior. The candidate serializes it identically and the report still contains the three built inferred fields, so inference construction and coverage are unchanged.

### Unit-test inventories

The complete baseline and final inventories matched after applying only the namespace mapping above:

| Profile | Baseline tests | Final tests | Ignored tests | Normalized inventory equal |
|---|---:|---:|---:|---|
| Default | 621 | 621 | 16 | Yes |
| All features | 622 | 622 | 16 | Yes |

No test name was added or omitted beyond its specified module-prefix move. Each ignored test retained its ignored status in both profiles. The four root cross-subsystem suites remained registered and ran in the final unit suites.

### Cleanup

No subordinate worktrees or implementation branches were created. The integration worktree is retained for owner review. The original checkout remains on `master`; its untracked task document is reconciled by this completed copy and is removed only after this report is committed and pushed.
