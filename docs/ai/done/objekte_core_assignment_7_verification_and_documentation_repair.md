# Assignment 7 repair — Ordered-query verification and public documentation

**Integration branch** `integration/core-repository-operations-and-state`  
**Reviewed delivery** `43005ca3e959d678614998ead6a76a444e7a62cc`  
**Baseline B7** `6f3c3ee8aa5a49cf82d83905b5eb212c1e984b46`  
**Original validated source** `0e8708fcfd302b356d026c344dd41491ed2cec89`

Continue the existing integration branch and sibling worktree. Preserve its history and existing benchmark artifacts. The original Assignment 7 specification remains the acceptance standard. The review accepted the repository split, the five root removals, the public query test and the strengthened A5 regression. The outstanding work concerns benchmark evidence and maintained documentation. A production change requires evidence of a specific behavioral difference.

## 1. Establish a valid cross-revision comparison

The original run directories are `bench/results/musenalm/repository-operations-{b7,final}`. I compared all 450 sample keys per side. Status and every recorded work counter agree. Of 150 ordered-result digests, 139 differ, exactly the 139 cases returning hits. The eleven empty pages agree. Running the repository's `bench/run_query_regressions.py::compatibility_failures` on those directories returns `corpus_fingerprint differs from baseline`. Both build gate records have `baseline: null`.

The two query manifests identify different physical worktrees as their corpus roots. In `crates/core/examples/bench_musenalm.rs`, `fingerprint_inventory` includes paths, modification times and sizes. `hash_paths` includes path spelling and content. Lines 396–413 set each cloned hit's generation to zero before comparing repetitions within a run. Lines 472–480 hash the count and ordered normalized hits. Independent invocations never compare their answers. The previous explanation attributing the digest differences to generation lacks support. Hit timestamps are a plausible cause that the original artifacts cannot establish.

1. Choose **one canonical absolute path** to the existing Musenalm corpus. Use that same directory, with the same files and timestamps, for every baseline and candidate invocation. Resolve the path once with `realpath` and record it. Keep this corpus read-only during the run. Keep both Git worktrees intact.
2. Build B7 and the repair candidate from their respective clean worktrees. Use distinct Cargo target directories. Set `OBJEKTE_BUILD_REVISION` to the full source SHA when compiling the examples and record the executable hashes, compiler, build flags and worker settings. Run each binary from its own worktree because the benchmark also records the execution checkout revision.
3. Run each release `bench_musenalm` binary using `--root "$corpus" --profiles ascii-off --repetitions 3 --query-workers 4 --verified-cache-bytes 0 --result-dir "$out"`. Set `corpus` as above and choose a distinct new output directory for each side. Run all 150 cases. Keep the resolved queries and schema manifests. A workload fingerprint can still reflect the compiled-in path to the workload file. Compare actual workload bytes and the resolved queries as well.
4. Repeat the matched build measurement for ASCII and Unicode, three samples each, using that same physical corpus. The build **executable** supports `--root`; the reviewed Python build driver does not forward it. Use the two release `bench_build_musenalm` binaries directly with `--root "$corpus" --alphabet ascii` or `unicode`, `--build-workers auto` and `--schema-manifest-path "$out/schema-manifest.json"`. Create the output directory first. Save the direct JSON samples and calculate the median wall time and peak RSS using the existing driver's definitions. An isolated benchmark-only option forwarding the root to the existing driver is also sufficient if it preserves the default. Record hashes and actual comparisons for both alphabets, including signature mismatches. An invocation with no baseline establishes only single-run checks.
5. Run the existing compatibility function on the new query directories and require an empty failure list. Check equal case sets, successful status, ordered result digests including count, page size, and all counter fields on all 450 matched samples. Store a compact machine-readable comparison with both revisions, corpus fingerprint, sample counts, mismatch counts and the first differing case for each category. Keep the raw evidence on the integration branch. No new performance threshold is needed. Explain material time or memory differences in the context of repeated measurements.
6. If every digest agrees under the common root, record 150/150 and finish this gate. If a digest still differs, print field-level differences for the first affected case and position using identical benchmark-only observation on both sides. Compare count and the **ordered sequence** of collection/record IDs, locators and byte ranges, timestamps, evidence, status, diagnostics and payloads. Check public ID/diagnostic references against their pinned catalogs if numeric handles differ. Compare any remaining affected cases before drawing a conclusion. Preserve order and duplicates. Blanket removal of metadata, sorting of the page, or a comparison of counts alone cannot establish parity. Fix a demonstrated observable regression with a focused red/green test. If only a proven revision-local representation differs, record the raw and resolved comparisons and a concrete example. Record unresolved differences as pending.

## 2. Repair maintained documentation

1. In `docs/inference.md:10–29`, import `CollectionDefinition` and `RepositoryOptions` from `objekte_search_core::repository::configuration`. Import `SourceRegistry` from `objekte_search_core::source::registry` and `SourceFormat` from `objekte_search_core::source`. Use a result type compatible with `IndexError` for the build/wait/view operations. Bind the view before borrowing its inference report and use the report while the view is live. Compile the actual Markdown example in the repair checkout. A temporary Cargo example built against the local core is sufficient. Supply a real temporary collection if executing the example.
2. The module comment in `crates/core/src/repository/mod.rs:3` links to `../../../tests/inline_sort.rs`. That path resolves to the nonexistent `crates/tests/inline_sort.rs` in the source tree and is also unsuitable for published HTML. Replace it with an absolute permalink to the accepted test in commit `3bf4c1f7c9167bfed012fe7011c553bc72a16dd9`, or put a compilable public usage example directly in the module documentation. Verify the selected link target or run the doctest. Keep the existing compile-fail privacy test.
3. Search maintained docs and examples for obsolete direct, grouped and aliased imports. Leave historical specifications and benchmark records intact. In the Assignment 7 completion report correct “four `base_reference_` controls” to **three** and name them. The `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock` regression is a separate fourth control. Run the prefix and the separately named test with positive execution counts.
4. Correct the implementation report at `docs/ai/done/objekte_core_repository_operations_and_state_specification.md`. Keep the old run identified as superseded, explain what the new data establishes, and distinguish benchmark and test revisions. Record any remaining uncertainty about the cause of differences in the old runs.

## 3. Acceptance and handoff

Run formatting, workspace all-targets check, the public `inline_sort` tests, the focused reference tests, core doctests and workspace documentation on the corrected revision. Record how the public Markdown example was compiled. If a production source change beyond documentation is necessary, rerun the full Assignment 7 gate set, including all-feature tests and the ignored release compaction regression. For a documentation/evidence-only repair, reference the already tested code SHA for the full suite and label the new runs accurately.

Commit and push to the integration branch per `docs/ai/README.md`. Report the full tested candidate SHA, final delivery SHA, new comparison artifact paths, actual parity counts and any pending items. The owner merges after review.

[Benchmark hit comparison](https://github.com/Simon-Martens/objekte/blob/43005ca3e959d678614998ead6a76a444e7a62cc/crates/core/examples/bench_musenalm.rs#L396) · [Compatibility check](https://github.com/Simon-Martens/objekte/blob/43005ca3e959d678614998ead6a76a444e7a62cc/bench/run_query_regressions.py#L33) · [Completion report](https://github.com/Simon-Martens/objekte/blob/43005ca3e959d678614998ead6a76a444e7a62cc/docs/ai/done/objekte_core_repository_operations_and_state_specification.md#L237)

## Implementation report

### Completion metadata

- Task identifier: `core-assignment-7-verification-and-documentation-repair`
- Completion date: 2026-11-19
- Reviewed delivery: `43005ca3e959d678614998ead6a76a444e7a62cc`
- B7: `6f3c3ee8aa5a49cf82d83905b5eb212c1e984b46`
- Original validated source: `0e8708fcfd302b356d026c344dd41491ed2cec89`
- Tested repair candidate: `d235605417b037eca76ea62101074914ea77845d`
- Integration branch: `integration/core-repository-operations-and-state`
- Integration worktree: `/home/simon/source/objekte-integration-core-repository-operations-and-state`

This was a documentation and benchmark-evidence repair. The repository split
and query/public API behavior were already accepted and did not require a
production source change.

### Common-root query verification

The canonical corpus was resolved once to:

```text
/home/simon/source/objekte/examples/musenalm
```

Its sorted inventory metadata, covering relative paths, nanosecond modification
times and sizes for all 148,501 files, had the same SHA-256 before and after all
measurements:

```text
ce54312379a363b9a76203ba072967d4ec80aa3422eb76c31f83c942b1589901
```

Both revisions received this same `--root`. The corpus contains 148,486 source
records across seven configured collections. Its common benchmark corpus
fingerprint is
`9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`;
the common schema fingerprint is
`bf36ea2e88d1b455e28b0f5f36c71ebd03847b2a986cd3f1f2ea3302fd1eea08`.

B7 and the last source implementation were compiled from their respective
worktrees with tracked source clean, full build revisions and distinct target
directories:

```sh
OBJEKTE_BUILD_REVISION=6f3c3ee8aa5a49cf82d83905b5eb212c1e984b46 \
CARGO_TARGET_DIR=<b7-target> \
cargo build --release --locked -p objekte-search-core \
  --example bench_musenalm --example bench_build_musenalm

OBJEKTE_BUILD_REVISION=43005ca3e959d678614998ead6a76a444e7a62cc \
CARGO_TARGET_DIR=<candidate-target> \
cargo build --release --locked -p objekte-search-core \
  --example bench_musenalm --example bench_build_musenalm
```

Both binaries used `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, Cargo 1.99.1, the
release profile, default build flags and the performance CPU governor. Relevant
binary SHA-256 values:

| Side | Query benchmark | Build benchmark |
| --- | --- | --- |
| B7 | `edd3d97f95683797b9e6f9d6202a7cdc0ef55d88ca455b5b0a68d295f2b6e80a` | `b6f82a81302b8adfb2d4f1df11f1d13d6b48351376b4d36d9ee267318ec20841` |
| Candidate | `16a15606e0ad51c71d607c9c869f8c5b7fa97e15204098468d7719eeec7bbfd6` | `5c85513a3b7ec8d977959f5c84a7370a88075297aa9d33905969d86f7809beab` |

Each side ran all 150 cases, three times each, with four query workers, ASCII,
folded-off caching disabled and the common root:

```sh
bench_musenalm --root /home/simon/source/objekte/examples/musenalm \
  --profiles ascii-off --repetitions 3 --query-workers 4 \
  --verified-cache-bytes 0 --result-dir <side>/queries
```

The compiled workload fingerprints differ because that field includes the
workload path compiled into each checkout. The workload files themselves are
byte-identical:

```text
56f2ba99e3a889b95975a6b460c4ef894732ad769f99e6f69086a3f0d3ac91c0
```

The resolved `queries.json` and schema manifests are byte-identical. The existing
`run_query_regressions.compatibility_failures` function returned an empty list.

The common-root comparison established:

- 450 baseline samples and 450 current samples; 450/450 sample keys matched.
- 150/150 ordered-result digests matched, including all 139 non-empty pages and
  all 11 empty pages.
- Status, error state, count, returned page size and every recorded work-counter
  field matched for all 450 paired samples.
- Both runs reported zero benchmark failures.

The ordered digest includes ordered collection/record identity, locator and byte
range, count, evidence and typed payload data; it was not sorted or stripped of
metadata. The comparison tool preserves the sequence and duplicates by comparing
the runner's ordered hit identity directly.

The compact machine-readable result is:

```text
bench/results/musenalm/repository-operations-common-root/comparison.json
```

Raw B7 and candidate query results, plans and 450-sample measurements are kept
under `repository-operations-common-root/{b7,candidate}/queries/`.

### Common-root build verification

The reviewed Python build driver does not forward `--root`, so the release
executable was called directly. The output directory was created and three runs
were made for each alphabet:

```sh
bench_build_musenalm --root /home/simon/source/objekte/examples/musenalm \
  --alphabet <ascii|unicode> --build-workers auto \
  --schema-manifest-path <side>/build/schema-manifest.json
```

Each alphabet produced 148,486 records and parses with zero failed parses on
both sides. Medians use the existing build driver's arithmetic-median definition:

| Side | Alphabet | Median | Range | Peak RSS |
| --- | --- | ---: | ---: | ---: |
| B7 | ASCII | 0.643 s | 0.635–0.647 s | 624.1 MiB |
| Candidate | ASCII | 0.635 s | 0.620–0.635 s | 617.2 MiB |
| B7 | Unicode | 0.736 s | 0.701–0.737 s | 629.7 MiB |
| Candidate | Unicode | 0.682 s | 0.682–0.699 s | 627.5 MiB |

The index signatures agree across revisions:

```text
ASCII   4a38808629a1544a354d715e42c85ac426b1cfe93376e862253b3d5cc03f349f
Unicode 5c235a33b6cb1a18c1a1f4d2b1d60fa1eae912ce2b4ff868857df95028d83868
```

The ASCII median was 1.3% lower, the Unicode median 7.4% lower, and peak RSS was
slightly lower for both. No new performance threshold applied. Build summaries,
binary identities and signature comparisons are in:

```text
bench/results/musenalm/repository-operations-common-root/build-comparison.json
```

The direct six-sample inputs are preserved under
`repository-operations-common-root/{b7,candidate}/build/`.

### Superseded evidence

The original `repository-operations-{b7,final}` query runs remain in history as
superseded evidence. They used two different physical corpus roots, produced
different corpus fingerprints, fail the existing compatibility check with
`corpus_fingerprint differs from baseline`, and differed for all 139 non-empty
result digests while the 11 empty digests agreed. Their manifests include file
timestamps, so timestamps are a plausible cause, but those artifacts do not
establish that this was the only or complete cause. The replacement common-root
run establishes complete ordered and work-counter parity without relying on that
uncertainty.

### Maintained documentation

`docs/inference.md` now imports:

- `CollectionDefinition` and `RepositoryOptions` from
  `objekte_search_core::repository::configuration`
- `SourceRegistry` from `objekte_search_core::source::registry`
- `SourceFormat` from `objekte_search_core::source`

The example returns `Result<(), IndexError>`, identifies `IndexError` from the
lifecycle module, binds `IndexView` before borrowing its inference report, and
uses the report while the view remains live. The exact example was copied into
an isolated Cargo project with a path dependency on the candidate core and
compiled successfully using `cargo check --offline --locked`.

The repository module documentation now links to the accepted `inline_sort.rs`
test at commit `3bf4c1f7c916...` with an absolute GitHub permalink. The target
was verified in the Git object store. The compile-fail privacy doctest remains.

A scan of maintained docs and examples found no obsolete direct, grouped or
aliased imports for the moved repository/configuration/query-result types.
Historical specifications and benchmark records were left intact.

The original Assignment 7 report now names the three `base_reference_` controls
and identifies `segmented_reference_parity_with_retired_and_new_rows_and_parameter_clock`
as the separate fourth control. It also identifies the old benchmark result as
superseded and records the common-root evidence.

### Validation

The existing full behavioral gate remains the result validated at
`0e8708fcfd`: workspace tests, all-feature tests, public caller tests, lifecycle
groups, and the 262,144-record release compaction regression all passed. This
repair changed documentation, benchmark-only comparison tools and committed
evidence, so it did not rerun those behavioral suites.

The repaired candidate `d235605417b037eca76ea62101074914ea77845d` passed:

| Command or check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | passed |
| `cargo check --locked --workspace --all-targets` | passed |
| `cargo test --locked -p objekte-search-core --test inline_sort` | 5 passed |
| `cargo test --locked -p objekte-search-core --lib relation_query::tests::base_reference_` | 3 passed |
| Exact separate segmented reference parity test | 1 passed |
| `cargo test --locked -p objekte-search-core --doc` | 1 compile-fail doctest passed |
| Workspace rustdoc with broken intra-doc links denied | passed |
| Corrected Markdown inference example in an isolated Cargo project | `cargo check --offline --locked` passed |
| Common-root ordered-query comparison | 450/450 samples and 150/150 digests matched |
| Common-root direct build comparison | 3+3 samples per side; signatures matched |

### Commits, uncertainty and cleanup

The repair candidate is
`d235605417b037eca76ea62101074914ea77845d`. It adds the documentation
corrections, report correction, comparison tools and common-root raw evidence.
The final report commit is separately identified in the handoff.

No production behavioral difference was found, so no production fix or new
red/green test was required. There are no pending query-parity differences. The
only unresolved historical question is the precise cause of the superseded
different-root digests; the new same-root evidence no longer depends on that
cause.

No subordinate worktrees were created. Temporary task memory is removed after
this report is pushed. The original Assignment 7 integration branch and worktree
remain for owner review and merge.
