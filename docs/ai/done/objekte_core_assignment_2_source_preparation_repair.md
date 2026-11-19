# Objekte core · Assignment 2 repair specification

**Review decision.** Accept the production moves. Repair the identity tests and the source-read evidence before signing off Assignment 2. The implementation preserves the six preparation operations, the three inherent JSON conversion methods and the six relocated test bodies. The only signature-text difference in those operations is a fully qualified spelling of `ExtractedRecord`. The three production callers use the new declared-format predicate. I found no new source read, parse or value clone in those moves.

| Revision | Commit |
|---|---|
| Series baseline B | `0f1d5e5ffa566a911f926ffe564e49c0c69ded77` |
| Accepted Assignment 1 | `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee` |
| Assignment 2 production source C | `c34294489f0011ca7a0009b54e0af5a6d6b600b3` |
| Reviewed Assignment 2 delivery | `b979e00d7caaad00a6d0fb680b4c1bab6c75a60d` |

Continue this assignment in `/home/simon/source/objekte-integration-core-refactoring-completion` on `integration/core-refactoring-completion`. Confirm the reviewed delivery is an ancestor of the current head. Preserve later work if the branch has advanced. Production paths below are relative to `crates/core/src/`. The evidence directory is `docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/`.

## 1. Make the identity matrix capable of detecting wrong locators

The new test in `query_execution/preindex.rs` sorts both result sets and compares only ID strings. Its JSONL assertion feeds the returned byte start back into the expected ID. An error shared by both engines can therefore pass. It checks that *some* JSON record is fallback, and its construction-error assertion accepts any path containing `data/`. The nested filename contains a literal backslash, which becomes a directory separator on Windows. The hard-coded expected ID also assumes the Unix path encoder. [Reviewed test](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/crates/core/src/query_execution/preindex.rs#L530-L683) · [Native path encoding](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/crates/core/src/model/path_representation.rs)

Move the pre-index test module to `query_execution/preindex/tests.rs` with `#[cfg(test)] mod tests;` so the existing qualified names still resolve. Use native path components and a portable nested filename with a literal percent. Keep the literal-backslash case under `#[cfg(unix)]`. For portable path IDs, pass an independently specified collection-relative path through the existing path encoder. Retain a literal expected Unix escape and the existing platform encoder tests.

Use these independent expected rows for the current ASCII JSONL fixture. Each range is half-open and excludes the line terminator.

| Source bytes | Byte range | Class | Identity |
|---|---|---|---|
| `{"title":"first"}` | `0..17` | `Logical` | Generated at byte 0 |
| `{"title":"second"}` | `18..36` | `Logical` | Generated at byte 18 |
| `{"title":"broken"` | `37..54` | `RawFallback` | Fallback for the complete range |

- Run indexed search and `SourceScan::search` against the same files. For each entry point check hit count, native repository-relative file path, both offsets, class and full ID against the fixture. Supply the observed file ID and revision time to `RecordId::generated`. For malformed rows use `RecordId::fallback` with the collection, resolved source ID and version, native repository-relative path, file ID, revision and expected bounds. Verify logical fallback delivery carries its diagnostic reference. For whole-file JSON use `0..fixture_bytes.len()`; check the two logical files and the specific broken file.
- Check returned order directly wherever the query contract defines a common order. A keyed lookup against expected rows is appropriate for locator validation. Make multiplicity explicit so an extra or missing hit fails.
- Exercise a successful folder-default query for `Registered("json")` in the **built-in** registry through both entry points. Assert generated IDs even though the files use the built-in JSON reader. Assert `ResolvedStrings` on that resolved handle. In a fresh registry with a custom reader under `json`, assert generated identity on a successful indexed query and `scalar_verification() == None`. Reuse the indexed/reference custom-reader controls. `SourceScan::open` provides its own built-in registry.
- For rejected overrides, isolate one valid file per case. Check that the indexed error contains the specific native filename, exact `start..end` and identity error. Check that pre-index opening succeeds and the search returns its existing identity error. The pre-index error has a different context contract.
- Correct the report's claim that `TinyReader` opts into trusted extraction. It inherits the default `false`. To demonstrate the interaction, reuse an opt-in reader in a fresh registry under `json`. Verify the resolved handle opts in, has no built-in scalar operation, and a **declared** `Registered("json")` still rejects the override on a valid record. The existing tracking-reader test already covers trusted versus validating dispatch.
- Clarify one assumption in the original matrix. The declared enum and installed reader are independent. A fresh registry can also resolve **declared** `Json` to a custom implementation under `json`. The declared format still permits path identity. With multiple logical records in a file, path IDs coincide and indexed admission has a duplicate-ID check. Document this compatibility boundary and retain the declared-format rule. The `Registered("json")` cases above cover a different declaration.

## 2. Make the comparison verifier reject incomplete evidence

I ran the committed `verify.py`. The real inputs reproduce the committed `comparison.json` byte for byte. I also supplied both revisions with empty result arrays, the same missing query, and three copies of `exact_page`. All three invalid inputs exited successfully and wrote `results_equal: true`. With `python3 -O`, a changed candidate count passed because Python removed every `assert`. [Verifier](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/verify.py)

Validate each revision independently. Require exactly one result for each of `exact_page`, `combined_old` and `combined_number`. Reject duplicates, missing or unknown names, absent fields, unexpected ordered IDs and incorrect counts. Use explicit failures that identify revision, query and field and survive optimized Python. Compare the validated revisions, then write the successful comparison. Exercise the real files plus paired empty, missing, duplicate, unknown and one-sided altered inputs in temporary directories. Every invalid case must exit nonzero and produce no new success result.

Derive each tested SHA from the checkout used to execute the probe and validate it against the intended revision. The existing program echoes an arbitrary `--tested-sha`. Update the reproduction commands so fixture creation, both checkouts, raw-output locations and verifier input locations work from an explicitly named directory. The four recorded fixture hashes reproduce. The generator currently leaves mtimes at write time although the README gives a fixed timestamp. Set and check the documented timestamp, or document that both executions share one fixture instance and record its actual metadata for that run.

## 3. Measure source reads at the boundary the assignment claims to verify

The four-file fixture yields the following public counters **at both revisions**. The exact query returns a logical payload from `00.json`, a 60-byte file. `WorkCounters::distinct_files` is documented as the number of physical primary-source files opened at least once. The two combined queries have no relation targets.

| Query | Count / returned | Reported opens | Reported distinct files | Reported bytes | Reported parses |
|---|---:|---:|---:|---:|---:|
| `exact_page` | 2 / 1 | 1 | 1 | 0 | 1 |
| `combined_old` | 3 / 3 | 3 | 6 | 165 | 3 |
| `combined_number` | 2 / 2 | 3 | 5 | 167 | 3 |

Five or six unique paths are impossible on this fixture. The segmented winner-materialization code reads selected source ranges, records selected-payload work separately and adds the size of a selected-file set to an existing distinct count. Equality of the public counters establishes parity with A1. It cannot establish how many unique files were opened or how many bytes the complete query requested. This discrepancy predates the move. [Counter definitions](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/crates/core/src/measurement/mod.rs#L48-L65) · [Segment payload read](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/crates/core/src/query_execution/runner/mod.rs#L441-L480) · [Winner accounting](https://github.com/Simon-Martens/objekte/blob/b979e00d7caaad00a6d0fb680b4c1bab6c75a60d/crates/core/src/query_execution/runner/mod.rs#L2854-L2925)

1. Retain the historical outputs. Label the existing fields as reported counters and include `selected_payload_reads` and `selected_payload_bytes` in the new results. Keep the expected ordered IDs and counts as semantic checks. Treat the inherited 5/6 distinct counts and zero bytes as observations in the historical data, never as correctness requirements for physical I/O.
2. On A1 and the repaired source, run the same three queries with one query worker, zero verified-cache bytes and the same files. Capture successful source opens and source reads for the query interval only. Use a test-only observer or a narrowly filtered OS trace. Limit to the four fixture paths, exclude indexing and reference queries, retain repeated reads, and record path, offset and byte count. Derive distinct paths by set union. Compare per-query opens, range multiplicities and byte totals across revisions. The exact query must return the expected logical value and read the selected record without reading the other matching file for payload.
3. The existing `source/access/mod.rs` test observer is thread-local and instruments `read_record_range_for_search`. Grouped/session reads have separate paths. Confirm observer coverage of every path exercised, or use the OS trace for those paths. Describe requested ranges and successful userspace reads precisely. Keep `record_parses` and the existing isolated combined-scan control as separate evidence. An unexplained candidate-only increase in reads or parses requires investigation before handoff.
4. Record the inherited unique-file accounting defect with this four-file reproduction in the completion report and the repository’s bug-issue workflow if no equivalent issue exists. This repair can close on independently verified A1 parity. A change to public counter semantics is a separate maintenance decision.

## 4. Correct the formatting and ownership record

- The report calls the spacing failure the same baseline. Its own totals are 198 files at A1 and 199 at C. I ran the repository's `spaced()` transformation on the new `model/json.rs`. It requests an additional blank line before each `impl`. Apply the local convention where the two required checks agree. If plain rustfmt and the repository spacing script disagree on these lines, record both exact outcomes and the affected paths. Keep unrelated baseline files out of the repair diff.
- Update `json-use-inventory.md`. `Number::parse_json_literal` consumes numeric token text from the query parser. `model/mod.rs` both renders and decodes quoted field-path segments. The archived-counter example in `measurement/mod.rs` is test coverage. Explain the built-in and fresh-registry `Registered("json")` cases explicitly. These corrections preserve the conclusion that whole source records reach their JSON decoder through the resolved reader.

## Verification and handoff

Run the repaired identity tests by exact qualified name, the existing combined-scan, borrowed-verification, exact-page and custom-reader controls, and the positive and negative verifier cases. When a Windows runner is available, run the focused portable identity test there and record the result. Identify the platforms actually executed. Run the original Assignment 2 final gates, including the default and all-feature core suites, non-JSON integration test and doc test. Record the spacing delta against A1 and the added test names. The reviewed delivery reports 608 and 609 passing core tests respectively, with 16 ignored in each profile. These are the agent's recorded results. This review independently executed the Python verifier, regenerated the fixture hashes and checked the new model file's spacing. Rust was unavailable in the review environment.

Commit the test and evidence changes on the series branch, then append the repair results to `docs/ai/done/objekte_core_assignment_2_source_preparation.md`. Record the actual tested source SHA and the final delivery SHA in the handoff. Push the integration commits under the existing workflow. The full-corpus benchmark and merge remain at their existing series milestones.

## Implementation report

- Task identifier: `core-refactoring-completion-source-preparation-repair`
- Completion date: 2026-11-19
- Series baseline B: `0f1d5e5ffa566a911f926ffe564e49c0c69ded77`
- Accepted A1: `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee`
- Reviewed A2 delivery: `b979e00d7caaad00a6d0fb680b4c1bab6c75a60d`
- Tested repair source: `d07b700a7188efd6b45ec542f197b78a211cb88a`
- Integration branch and worktree: `integration/core-refactoring-completion` /
  `/home/simon/source/objekte-integration-core-refactoring-completion`

### Identity corrections

The inline pre-index test module moved to `query_execution/preindex/tests.rs`
without changing the qualified names of its retained test. It now accepts
independently specified expected rows and checks collection, native relative
path, both byte offsets, class, full identity and multiplicity for indexed and
pre-index execution.

The JSONL fixture uses exact half-open ranges `0..17`, `18..36` and `37..54`;
the two logical IDs use observed file ID and revision time, and the malformed
row uses `RecordId::fallback`. Both entry points are compared directly, and
logical fallback delivery is checked for a diagnostic reference. Whole-file
JSON checks complete ranges, a portable literal-percent path through the path
encoder, the specific broken file and a Unix-only literal-backslash ID.

The successful built-in `Registered("json")` path asserts generated IDs and
`ResolvedStrings`. A fresh opt-in reader under `json` confirms trusted
extraction with no scalar operation while explicit relative-path identity is
rejected. The custom multi-record declared-`Json` collision is tested and
documented. Rejected JSONL and registered JSON overrides isolate one valid
file and assert filename, exact range and distinct construction/search stages.

### Verifier and I/O evidence

`verify.py` now validates each revision independently with explicit exceptions,
requires exactly one of each expected query, rejects duplicates/missing/unknown
names and absent or changed fields, and validates the compact OS-trace files.
`python3 -O verify.py --self-test` exercises paired empty, missing, duplicate,
unknown and one-sided altered inputs in temporary directories. Invalid cases
cannot write a successful comparison.

The probe derives its tested SHA from `git rev-parse HEAD` and fails unless it
matches the intended revision. Both executions share the same generated fixture
whose source mtimes are fixed to Unix timestamp `1795097068`. `strace` records
successful opens, reads, offsets and byte counts only within per-query marker
intervals for the four fixture paths.

At A1 and the repaired source, `exact_page` opened `00.json` once and read 60
bytes from offset zero. `combined_old` made six successful opens across three
paths and read 330 bytes in six ranges. `combined_number` made five opens
across three paths and read 286 bytes in five ranges. Ordered opens, ranges and
totals are identical across revisions.

The public historical counters remain unchanged: `exact_page` reports 1/1/0
opens/distinct/bytes, `combined_old` 3/6/165, and `combined_number` 3/5/167.
Selected payload counters are now recorded separately. The inherited accounting
defect is documented in
[`query_source_counter_semantics.md`](../issues/bug/query_source_counter_semantics.md).

### Validation and formatting

The repaired identity tests passed by exact qualified name:

- `relative_path_identity_checks_whole_file_ranges_and_fallback_ids`
- `jsonl_identity_checks_exact_ranges_classes_and_fallback_identity`
- `registered_json_uses_generated_ids_with_builtin_scalar_verification`
- `relative_path_errors_name_one_valid_file_and_exact_range`
- `source_scan_matches_reference_search_before_an_index_exists`
- `registered_readers_do_not_enable_relative_path_identity`

The combined-scan, borrowed-verification, exact-page and custom-reader controls
also passed. Final gates passed:

| Command | Outcome |
|---|---|
| `cargo check --locked --workspace --all-targets --all-features` | Passed |
| `cargo test --locked -p objekte-search-core --lib` | 611 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --lib --all-features` | 612 passed, 16 ignored |
| `cargo test --locked -p objekte-search-core --test non_json_relations` | 1 passed |
| `cargo test --locked -p objekte-search-core --doc` | 1 passed |
| `cargo clippy --locked --workspace --all-targets --all-features` | Passed |
| `python3 -O verify.py --self-test` | Passed, including all negative cases |

The four repaired identity test names replace the reviewed
`relative_path_identity_matches_indexed_and_preindex_paths` test, increasing
the suite by three. Ignored status remains 16 in each profile. Tests and traces
ran on Linux; no Windows runner was available.

Plain `cargo fmt --all -- --check` requests one blank line around the two
`impl` blocks in `model/json.rs`. The repository spacing script accepts the
local two-blank-line convention there but continues to reject its inherited
199-file baseline. Both exact outcomes are recorded rather than mixing the
unrelated baseline files into this repair.

### Cleanup and handoff

The detached A1 worktree and temporary fixture/trace directory were removed
after the compact evidence was saved. The integration branch and worktree remain
for owner review. Full-corpus benchmarking remains an Assignment 3 milestone.
