# Assignment 4 repair specification — Source registration and scalar scanning

**Start** at A4 `5b3804be019ddb55447c7ad7810786e801a553a5` on `repair/core-source-registration`. Original base B is `1eef51b12c9dad55c2b9b7e603b888412cb51ad5`. This is the first repair in the stack.

## Resolve the capability once

At A4, crates/core/src/source/registry.rs:23 stores a registration and a Boolean. At :46 the accessor reinterprets the registration string. The accessor is called from per-candidate verification (query_execution/runner/segment_scan.rs:82) and record preparation (:183). The custom registration fallback itself is correct.

Replace the private Boolean with a private copyable provenance enum with variants Extension and Builtin(ScalarVerification). Classify once in SourceRegistry::resolve, using trusted installed provenance, with json and jsonl mapping to ResolvedStrings and json_decoded_scalars to DecodedScalars. Keep is_extension and scalar_verification as constant-time projections of that field. Keep the existing public reader trait, registration structure, duplicate checks, fingerprints, serialization and identity rules. An extension installed under any of the three names must produce None, including when the trait name advertises a built-in. Do not infer capabilities from suffix, format enum, name or coverage. Do not introduce a second registry lookup in any query loop.

## Finish scalar ownership

Move CombinedFts and its impl (source/json/scalars/mod.rs:58), borrowed events, scratch, scratch trimming, worker-local scanner storage, counters, scan entry and combined projection-plus-FTS entry (:521–612, :655 onward) into scalars/scan.rs alongside the borrowed walker. Keep ordinary scalar construction/projection and walker state in mod.rs. Keep quoted-string decoding in strings.rs. Give siblings only the visibility needed for shared walker operations. Update verification.rs to call the actual implementation and move the existing owned result/evidence across the boundary. Keep exactly one thread-local scratch allocation domain, current trim behavior on success and errors, and a single traversal for fused reads. Move decoder and scanner tests from scalars/tests.rs to owner-local suites, preserving both ignored benchmarks, private fixtures, names and assertions. Use the actual session/mod.rs path in the amended report.

## Acceptance evidence

Extend source/registry/tests.rs:147 to exercise jsonl in addition to json, json_decoded_scalars and the neutral control. Make at least one reader report a built-in name. Assert installed capability for each real built-in and absence for the custom readers. Exercise the public pinned search through non-JSON bytes, field-scoped FTS, unindexed residual FILTER, their intersection, positive and negative hits, evidence, and locator and logical output. Use fresh requests or disable the verified-result cache when demonstrating the paths. Include an explicit expected record ID and count independent of reference evaluation; assert the reader actually runs for residual verification. Keep original-record-bytes extension support and eligibility for complete reader-owned text.

On equivalent built-in JSON and decoded-scalar fixtures compare B and the repair using existing parse, borrowed/fused traversal, owned-segment, I/O and candidate counters. Record actual observed values or the named assertion with its expected value, input and matched test result for each. Preserve all seven A4 root removals. Any parser-loop, buffering or ownership change beyond a move needs a same-input focused release comparison. Record R4 for Assignment 5.

## Integration and acceptance

Use a sibling worktree under /home/simon/source and a new repair branch. Preserve master and all three frozen delivery refs. Record the actual starting commit, original comparison, implementation commits and tested code SHA. Keep the original specification authoritative for unchanged behavior. Use commits that build through normal workspace entry points. Deliver to the owner for review, without merging into master.

Run the following on the final code tree, retain command summaries and exit status, and compare default and all-feature test inventories by function name and multiplicity, accounting for module moves. For every individually filtered test, show the fully qualified test name and the positive number executed. Fix errors within scope before marking the repair ready. In the report distinguish results actually run from historical results. After a report-only commit verify that its only delta is the report and record the final full SHA.

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

## Implementation report

- **Task:** `core-source-registration-repairs`
- **Completion date:** 2026-11-19
- **Starting A4:** `5b3804be019ddb55447c7ad7810786e801a553a5`
- **Original B:** `1eef51b12c9dad55c2b9b7e603b888412cb51ad5`
- **Repair branch:** `repair/core-source-registration`
- **Repair worktree:** `/home/simon/source/objekte-repair-core-source-registration`
- **Tested code revision:** `56c193b53b205935ade143a1052ceb906be986e1`
- **Implementation commits:** `56c193b53b205935ade143a1052ceb906be986e1`
- **Original comparison:** [A4 to tested R4 code](https://github.com/Simon-Martens/objekte/compare/5b3804be019ddb55447c7ad7810786e801a553a5...56c193b53b205935ade143a1052ceb906be986e1)
- **Status:** complete and ready for review; no merge into `master` was performed

### Summary

`PreparedSource` now stores a copyable private `SourceProvenance` of either
`Extension` or `Builtin(ScalarVerification)`. `SourceRegistry::resolve`
classifies the trusted built-in identity once; `is_extension` and
`scalar_verification` are constant-time projections with no capability inferred
from the format enum, suffix, extension, reader name or text coverage.

The capability regression exercises custom readers under `json`, `jsonl`,
`json_decoded_scalars` and a neutral control ID. One custom reader advertises
the built-in `json` trait name. The test verifies that all custom installations
remain `Extension` with no scalar verifier while the three real registrations
resolve to their correct verifier. It runs public pinned searches over non-JSON
bytes with field-scoped FTS, unindexed residual FILTER, their intersection,
positive and negative hits, evidence, logical payloads and locators. It asserts
hand-derived IDs (`needle-alpha`, `needle-beta`), `needle-alpha` for the residual
filter, counts 2/1/1/0, source ranges, and a positive custom-reader invocation
for residual verification.

`CombinedFts`, borrowed events, worker scratch, trimming, worker-local storage,
scanner counters and both scanner entry points now live in
`source/json/scalars/scan.rs`. Ordinary construction, walker state and
projection remain in `mod.rs`; quoted-token decoding remains in `strings.rs`.
There is still exactly one thread-local borrowed-scan scratch domain and one
traversal for fused projection plus FTS.

### Test ownership

- `source/json/scalars/scan_tests.rs` owns borrowed scanner, scratch, fused
  projection, owned-segment and scanner-benchmark tests.
- `source/json/scalars/string_tests.rs` owns decoder behavior and the decoder
  benchmark.
- `source/json/scalars/tests.rs` retains ordinary construction, projection and
  walker tests.
- Both ignored benchmarks retain their original names and ignore reasons.
- All seven A4 root removals and R4 capability behavior remain present.

The actual session path checked and used by this A4 surface is
`crates/core/src/source/access/session/mod.rs`.

### B/R4 work comparison

The following existing assertions passed on both B and tested R4 unless noted;
each selected command executed exactly one test:

- `query_execution::runner::tests::borrowed_source_verification_matches_reference_for_outputs_and_duplicates`
  passed 1/1 on both. Input was escaped and duplicate JSON/JSONL records at
  startup size as defined by that test. It matched reference output across all
  return modes and asserted expected count 2, a positive borrowed-scan delta
  for locator/no-evidence verification, and zero additional owned collector
  text bytes for built-in JSON and JSONL.
- `query_execution::runner::tests::root_scoped_fts_matches_reference_across_readers_and_outputs`
  passed 1/1 on both. It asserted the fixture's hand-derived counts, including
  5 for `needle` over root/title scopes and the decoded-scalar-only unscoped
  result established by the test, across JSON, JSONL and decoded scalars.
- `query_execution::runner::tests::decoded_scalar_fts_combines_source_text_with_required_values`
  passed 1/1 on both. It asserted counts 3 and 2 for `old` and `1e0`, a positive
  combined-scan counter delta, and `record_parses == candidates`.
- `query_execution::runner::tests::selective_candidate_and_broad_exact_or_share_the_restricted_domain`
  passed 1/1 on both. It asserted the restricted results and counts 1, 2 and
  951; at most 2 files/residual evaluations for selective conjuncts; equality
  of record parses and files opened for shared residual terms; and at most 50
  files for the broad no-evidence OR case.

R4 scanner tests additionally retained the existing assertions that the fused
JSON path performs one fold-transform candidate per segment and zero owned text
bytes, filter-only irrelevant-container pruning visits fewer than 10 nodes,
worker scratch reuses its allocation and caps retained storage at 256 KiB, and
oversized scan errors release scratch. No parser loop, buffer ownership or I/O
schedule changed beyond moving them to their owner. No additional release
comparison was required.

### Validation actually run

The following commands passed at tested code revision
`56c193b53b205935ade143a1052ceb906be986e1`:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo test --locked -p objekte-search-core --all-features
RUSTDOCFLAGS='-D rustdoc::broken_intra_doc_links' cargo doc --locked --workspace --no-deps
```

Workspace results included CLI 81 passed; process integration 33 passed and
one ignored; desktop 161 passed and one ignored; all core integration suites;
and one passing core compile-fail doc test. Default core was 583 passed and 16
ignored. All-feature core was 584 passed and 16 ignored.

Individually filtered commands and positive execution counts:

- `cargo test --locked -p objekte-search-core --lib source::registry::tests::custom_readers_under_builtin_names_never_use_builtin_scalar_verification -- --exact --nocapture`
  — 1 passed, 0 failed, 598 filtered.
- `cargo test --locked -p objekte-search-core --lib source::json::scalars::`
  — 22 passed, 2 ignored, 0 failed, 575 filtered.
- The four B/R4 comparison tests above — each 1 passed, 0 failed.

Default and all-feature inventories were compared against A4 after normalizing
module prefixes and retaining function-name multiplicity. Both sides contain
628 default entries and 629 all-feature entries, and both diffs are identical;
the scanner test move added, removed and renamed no test function.

### Review notes

No unresolved issue or blocked check remains. The report-only commit follows
tested code revision `56c193b53b205935ade143a1052ceb906be986e1`; its only
delta is completion documentation. The branch and worktree remain for the
owner's review. Assignment 5 is based on this accepted R4 result.
