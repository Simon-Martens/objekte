# Objekte core · Assignment 1 repair

Close the two findings from the module-ownership review. Demonstrate that three real queries consume the published inferred postings on the 32-record fixture. Move six implementation files to the paths specified in Assignment 1. Deliver the repair on the existing integration branch for review before Assignment 2.

| Revision or location | Value |
|---|---|
| Original series baseline, B | `0f1d5e5ffa566a911f926ffe564e49c0c69ded77` |
| Reviewed Assignment 1 source | `587835f5a08982fa22908adec9481952c7501a5d` |
| Reviewed Assignment 1 delivery | `b959208da172c8edb03148a8f981440cf078e6e9` |
| Integration branch | `integration/core-refactoring-completion` |
| Integration worktree | `/home/simon/source/objekte-integration-core-refactoring-completion` |

Read `docs/ai/README.md` and the completed Assignment 1 report. Continue from the published delivery, preserving any later work on the branch. Use an existing checkout at B or a detached sibling worktree under `/home/simon/source` for the comparison. Keep the original checkout on `master`. Push each repair commit.

## 1. Restore the specified source paths

Apply these six `git mv` operations under `crates/core/src/`. Commit them together as `Align core module files with the ownership specification`. Record the resulting source SHA as **C**.

| Existing implementation | Required implementation | Existing tests to retain |
|---|---|---|
| `model/decimal/mod.rs` | `model/decimal.rs` | `model/decimal/tests.rs` |
| `model/path_representation/mod.rs` | `model/path_representation.rs` | `model/path_representation/tests.rs` |
| `model/structure/mod.rs` | `model/structure.rs` | `model/structure/tests.rs` |
| `inventory/discovery/mod.rs` | `inventory/discovery.rs` | `inventory/discovery/tests.rs` |
| `measurement/timer/mod.rs` | `measurement/timer.rs` | `measurement/timer/tests.rs` |
| `measurement/format/mod.rs` | `measurement/format.rs` | `measurement/format/tests.rs` |

Rust resolves each existing `mod tests;` in the adjacent module directory. Move the implementation file only and retain its contents, parent declaration and test feature gate. Inspect `git diff --cached --name-status --find-renames=100%` before committing. Expect six `R100` entries. This gives an exact check that no runtime logic changed.

Run `cargo check --locked --workspace --all-targets` and the six relocated unit suites under their existing module names. Verify that `measurement::timer::tests::ordinary_build_has_no_detailed_clock_or_timer_state` executes in the default configuration and is absent with `--all-features`. A test-list comparison against the reviewed Assignment 1 delivery should have identical names and ignored status in both profiles. Its recorded inventories contain 621 default and 622 all-feature unit tests, including 16 ignored in each. Reuse the existing inventories when available.

## 2. Verify inferred postings through the published query interface

The previous fixture printed `indexes: null` on both sides. Its example reads `collection.inference`; the segmented build publishes the posting dictionaries on the segments. The three `Built` statuses establish admission. The check below also establishes which rows the dictionaries return and whether execution reads any source records to answer the queries.

**Set up the comparison.** Generate the original fixture once using the Python snippet in Assignment 1. Use that same physical `data/records.jsonl` at B and C, preserving its bytes and modification time. The 32 rows have `sequence` values 0 through 31. Even rows have `enabled = true` and `group = 'A'`. Keep the example's collection name `records`, JSONL reader, declared `manual` field, prohibited `blocked` field and default inference policy. Set `AutoIndex` and zero verified-cache bytes explicitly.

**Use one small probe at both revisions.** The simplest implementation is a temporary Cargo example modeled on `bench_inference_fixture.rs`. Keep its source with the repair evidence so the comparison can be repeated. The only expected source adaptation is the import of `ReturnMode`, from `execution_options` at B and `query::options` at C. Use `Index::start_build(...).and_then(|task| task.wait_complete())` as the existing example does. Obtain one `IndexView` from the completed build and use that same pin for diagnostics, explain and search. The existing public API supplies all three operations.

| FILTER | Expected count and page size | Expected JSONL rows, numbered from zero |
|---|---:|---|
| `enabled = true` | 16 | 0, 2, 4, …, 30 |
| `group = 'A'` | 16 | 0, 2, 4, …, 30 |
| `sequence = 31` | 1 | 31 |

For every query on each revision, require all of the following.

1. Build a single-term query scoped to `records`, with count requested, locator output, offset 0, limit 32, empty sort, no FTS and evidence disabled. Compare the reported count **and** the complete page of hits with the table.
2. Establish expected membership independently by walking the fixture's original bytes once in the probe setup and mapping each line to its row number and byte start. For the returned page, check collection, file and byte starts against the expected rows. Reject missing, extra or duplicate locators. Compare the original ordered locator sequence across B and C as well. The fixture line order need not be the query's default page order.
3. Find the trace for `records` from `view.explain(&query)`. For the applicable term require `term.selected.kind == AccessPathKind::Posting` and `term.selected.exact`. A posting listed among the alternatives is insufficient. These fixture fields are undeclared, so this proves selection of the inferred route.
4. From the actual optimized `view.search(&query)` result require `counters.files_opened == 0` and `counters.record_parses == 0`. Record the counters for each query separately. Fixture generation, index construction and the independent locator calculation occur outside the measured search.
5. Capture `view.inference_index_diagnostics("records")` for the three field paths. Record their status, value-key counts and coverage alongside the query results. Keep the original `indexes: null` observation identified as a limitation of the old example. The selected exact plans, returned members and work counters supply the missing proof.

Save compact baseline and candidate results under `docs/ai/generated/core-refactoring-completion/assignment-1-repair/`. Include both revision SHAs, fixture SHA-256, reproducible probe command, filter, selected access kind/exactness, counts, locators and work counters. The two runs produce **six observed query results**. Make the verification exit unsuccessfully on any failed assertion.

If a query fails at C, run the same probe at the reviewed Assignment 1 delivery to isolate the change. If B also fails, report the actual baseline plan, membership and counters. Preserve the failed result and identify an incorrect probe assumption or a pre-existing implementation issue before claiming acceptance. The 32-row fixture is sufficient for this repair. The scheduled Musenalm comparison belongs to Assignment 3.

## Handoff

Append a correction to `docs/ai/done/objekte_core_assignment_1_module_ownership.md` identifying the original fixture limitation and linking the replacement evidence. Complete this repair document under `docs/ai/done/` according to the repository workflow. Include the six rename results, focused test counts, the six-query verdict and the tested SHA C. Commit and push the reports and evidence as `Record Assignment 1 posting verification and layout corrections`. Give the owner the final pushed repair SHA as the Assignment 1 handoff for Assignment 2.

[Assignment 1 fixture and acceptance criteria](https://github.com/Simon-Martens/objekte/blob/b959208da172c8edb03148a8f981440cf078e6e9/docs/ai/done/objekte_core_assignment_1_module_ownership.md#L84) · [Fixture output implementation](https://github.com/Simon-Martens/objekte/blob/b959208da172c8edb03148a8f981440cf078e6e9/crates/core/examples/bench_inference_fixture.rs#L74) · [Existing exact-posting and zero-read assertions](https://github.com/Simon-Martens/objekte/blob/b959208da172c8edb03148a8f981440cf078e6e9/crates/core/src/inference/tests.rs#L415)

## Implementation report

- Task identifier: `core-refactoring-completion-assignment-1-repair`
- Completion date: 2026-11-19
- Reviewed delivery and base: `b959208da172c8edb03148a8f981440cf078e6e9`
- Repair source C: `9a234188f82e456f1e245fae07b179b64878b05a`
- Integration branch: `integration/core-refactoring-completion`
- Integration worktree: `/home/simon/source/objekte-integration-core-refactoring-completion`

The remote integration head was the reviewed delivery before repair work started. The original checkout remained on `master`; B and the reviewed delivery were checked in detached sibling worktrees for the query and inventory comparisons.

### Source layout

Commit `9a234188f82e456f1e245fae07b179b64878b05a` contains exactly six `R100` implementation-file renames:

| Existing path | Required path |
|---|---|
| `model/decimal/mod.rs` | `model/decimal.rs` |
| `model/path_representation/mod.rs` | `model/path_representation.rs` |
| `model/structure/mod.rs` | `model/structure.rs` |
| `inventory/discovery/mod.rs` | `inventory/discovery.rs` |
| `measurement/timer/mod.rs` | `measurement/timer.rs` |
| `measurement/format/mod.rs` | `measurement/format.rs` |

The adjacent test directories, module declarations and timer feature gate were retained unchanged.

Focused results at C were: decimal 3 passed, path representation 1, structure 1, discovery 4, timer 1 in the default build, timer selector absent in the all-feature build, and format 1. `cargo check --locked --workspace --all-targets` passed before the rename commit.

The complete reviewed-delivery and C inventories matched exactly:

| Profile | Tests | Ignored | Names and ignored status equal |
|---|---:|---:|---|
| Default | 621 | 16 | Yes |
| All features | 622 | 16 | Yes |

### Inferred-posting proof

The fixture SHA-256 was `9c76b850fca687ebfc24758b164f178de5237b7e93009f00f1054d65cc80fcbb`, with size 2,534 bytes and unchanged mtime seconds `1795095659` throughout both runs. The probe used one completed-build `IndexView` for diagnostics, explain and search.

| Revision | Filter | Plan | Count | Rows | Files opened | Records parsed | Scan/residual work |
|---|---|---|---:|---|---:|---:|---|
| B | `enabled = true` | exact posting | 16 | even rows 0…30 | 0 | 0 | 0 / 0 |
| B | `group = 'A'` | exact posting | 16 | even rows 0…30 | 0 | 0 | 0 / 0 |
| B | `sequence = 31` | exact posting | 1 | 31 | 0 | 0 | 0 / 0 |
| C | `enabled = true` | exact posting | 16 | even rows 0…30 | 0 | 0 | 0 / 0 |
| C | `group = 'A'` | exact posting | 16 | even rows 0…30 | 0 | 0 | 0 / 0 |
| C | `sequence = 31` | exact posting | 1 | 31 | 0 | 0 | 0 / 0 |

The ordered locator sequence, including collection, file ID, byte start and record ID, was identical across B and C. Each query rejected missing, extra, duplicate or wrong-file locators inside the probe. Diagnostics at both revisions reported `Built` with complete value dictionaries and 32 covered records: `enabled` had 2 value keys, `group` 2 and `sequence` 32.

The evidence is under [assignment-1-repair](../generated/core-refactoring-completion/assignment-1-repair/README.md). It contains both probe source variants, the fixture generator and checker, compact baseline and candidate results, the combined comparison, fixture fingerprint and reproduction commands. `python3 verify.py` exits unsuccessfully on any plan, membership, counter or cross-revision assertion failure.

### Final validation

At C:

- `cargo fmt --all -- --check` passed.
- `cargo check --locked --workspace --all-targets --all-features` passed.
- `cargo test --locked -p objekte-search-core --lib` passed with 605 tests and 16 ignored.
- `cargo test --locked -p objekte-search-core --lib --all-features` passed with 606 tests and 16 ignored.
- `cargo clippy --locked --workspace --all-targets --all-features` passed.
- The six-query probe and `verify.py` passed.

The fixture probe is the relevant execution benchmark for this repair. The broader 150-case Musenalm build/query comparison remains scheduled for Assignment 3.

### Cleanup

The detached B and reviewed-delivery comparison worktrees were clean and were removed after their results were saved. The temporary Cargo example copies and fixture directory were removed. The integration worktree and branch are retained for owner review.
