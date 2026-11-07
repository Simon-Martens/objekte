# Optional indexing, sorting, and streaming: integrated result

## Revision and scope

The implementation starts at `f374101b4839f28199a99677118ac96869203bb0`. The integrated core measured below is `ef44e42e566abe1a808094cd7933cc798a63f63d`; commit `5f9e221b0f` adds only a folder selector to the benchmark example. The accepted dynamic, inference, sorting, and streaming heads are `0fbf439aa9`, `a34154091a`, `4e3a6f4325`, and `4cb58e0d73`. They were merged in that order. Integration repairs are in `1dec9792ad` and the collected-scan overhead fix is in `ef44e42e56`.

The validated integration candidate `0ad2ecdd9a` was fast-forward pushed to `development/local-file-search-core`. The follow-up ledger update only records that completed promotion; it does not change measured code or workload.

The result includes a recursive schema-free folder definition, conservative bigram restrictions for undeclared string predicates, opt-in inference, computed and related sorting with bounded selection, and progressive streaming with cancellation and resource outcomes. Inference is **Disabled by default**. The Musenalm fixture is read through an isolated hard-linked copy; the user's original checkout and its TUI cache were not modified.

## Correctness

- `cargo test --workspace --all-targets`: passed at `5f9e221b0f`, including core, examples, and TUI. `cargo clippy --workspace --all-targets`: exited 0 with advisory warnings. The repository-wide `cargo fmt --all --check` fails on pre-existing spacing drift in unrelated files; changed Rust files were formatted and `git diff --check` passed.
- Final independent handwritten jq oracle: **300/300 complete ID sets** for Q001–Q150 over 148,486 original JSON files, ASCII and Unicode. See [jq-final/REPORT.md](jq-final/REPORT.md) and its per-query SHA-256 manifest. The oracle does not use the core parser, binder, scanner, index, or cache.
- Final independent raw-JSON relation oracle: **18/18 complete ID sets** for nine relation cases in both alphabets. See [relation-oracle-final/REPORT.md](relation-oracle-final/REPORT.md).
- The full optimized-versus-reference acceptance run covers all 188 previously recorded queries in both alphabets, including complete count, page, payload and evidence comparisons. Six appended computed/related sort cases Q189–Q194 also pass in both alphabets. See [acceptance-final](acceptance-final). Focused integration fixtures additionally cover projected relation targets, mixed dynamic/inferred plans, empty arrays, fallback records, restored snapshots, cached computed sorts, streaming replacement pages, cancellation and resource limits.
- The final 188-query timing run has **3,760/3,760 successful samples**. For Q001–Q150, result counts, returned page sizes, candidate counts, file opens, scanned records, and parses equal the recorded baseline in all four profiles. Q160 now uses a safe dynamic path: 133,948 to 70,390 scanned records, with the same 70,386 matches. Q162 depends on source modification time and changed with the copied fixture's metadata (4,928 to 0 matches); it is not a semantic regression.

## Established Musenalm workload

The old core tree at `f374101b48` matches the recorded `b70c7eef05` core. The historical run and the final run use the same 188 query definitions and schema manifest, Rust 1.99.1 release build, 32 query workers, zero source/decoded caches, ASCII/Unicode, verified cache disabled or 64 MiB warm, and five repetitions. The corpus file count and content are unchanged. **Source metadata fingerprints differ**, so timing comparisons with the historical run are limited, particularly metadata query Q162. Values below are sums of per-query medians, not a single batch latency. Absolute milliseconds precede percentages.

| Queries | Profile | Old ms | New ms | Difference ms | Difference % |
|---|---|---:|---:|---:|---:|
| Q001–Q150 | ASCII, cache off | 2,117.316 | 1,909.970 | -207.346 | -9.79% |
| Q001–Q150 | Unicode, cache off | 2,008.975 | 1,950.214 | -58.760 | -2.92% |
| Q001–Q150 | ASCII, warm | 1.442 | 1.454 | +0.012 | +0.80% |
| Q001–Q150 | Unicode, warm | 1.417 | 1.450 | +0.033 | +2.33% |
| Q151–Q188 | ASCII, cache off | 507.306 | 392.330 | -114.976 | -22.66% |
| Q151–Q188 | Unicode, cache off | 503.146 | 420.775 | -82.371 | -16.37% |

The complete [752-row per-query table](FULL_QUERY_TABLE.csv) includes full request JSON, five-sample min/median/max milliseconds, absolute and percentage difference, match/page counts, candidate counts, file opens, scanned records, and parses. It covers the 188 cases shared with the historical run. The separately appended Q189–Q194 have no historical equivalent and are recorded in [new-sort-cases](new-sort-cases). Raw samples, query manifest, plans, build/restore results and environment are in [timed-final](timed-final); historical samples are in `bench/results/musenalm/extended-b70c7eef05/timed/`.

The Q189–Q194 timing run uses the same four profiles, five repetitions, registered schema and isolated fixture, with zero failures. The table shows cold median milliseconds and actual source opens; these are **new operations**, so there is no old-core speedup percentage.

| Case and sort | Matches | ASCII ms | Unicode ms | Source opens |
|---|---:|---:|---:|---:|
| Q189 unregistered `edit_state` | 7 | 4.092 | 5.215 | 203 |
| Q190 `MIN(language)` | 3,821 | 79.365 | 81.225 | 3,821 |
| Q191 related `entry.preferred_title` | 95,153 | 1,748.187 | 1,756.753 | 96,921 |
| Q192 positional `contributors[0].type` | 70,386 | 1,252.783 | 1,298.536 | 70,390 |
| Q193 `FIRST(related_entries.entry.year)` | 18 | 14.607 | 16.581 | 1,311 |
| Q194 two-key local order | 7 | 0.315 | 0.291 | 203 |

Broad computed/related sorting evaluates many source keys: Q191 and Q192 take roughly 1.2–1.8 seconds cold, and even their warm-membership runs take about 0.66–0.97 seconds because order keys must be read and recomputed. This is an explicit cost of sorting 70–95 thousand matching records by nonresident values, not evidence that the old registered-column path slowed. The runner's plans, source counters and raw samples make that work visible.

A paired old/new process check found that Q006/Q007 initially regressed because stream buffer accounting ran during collected scans. The scoped fix skips that accounting when there is no stream delivery. In paired ASCII runs, Q006 old/pre-fix/fixed medians were 274.836/359.348/303.137 ms and Q007 were 378.313/481.593/387.689 ms; in Unicode, Q006 were 281.230/364.954/289.837 ms and Q007 were 366.084/499.707/343.788 ms. The final full run varies more on these broad scans, but its aggregate fixed workload has no unexplained material regression. See [focused-control-old](focused-control-old), [focused-control-new](focused-control-new), and [focused-scanfix](focused-scanfix).

## Build and restore

The registered-schema full corpus still builds 15 bigram stores with identical distinct gram-key counts and 148,486 successful parses. Historical versus final build time: ASCII 1,197.233 → 1,232.223 ms; Unicode 1,199.363 → 1,284.726 ms. Snapshot write: ASCII 255.179 → 356.054 ms; Unicode 268.480 → 369.332 ms. Five-sample restore medians: ASCII 502.514 → 546.541 ms; Unicode 506.900 → 559.823 ms. Final snapshot sizes are 117,262,305 and 119,772,881 bytes, about 41 KiB larger than before. Final peak RSS is 610 and 867 MiB, below the historical 624 and 874 MiB. These are separate historical and current process runs with different source metadata and cache conditions; the 35–85 ms construction and 44–53 ms restore differences are observations, not proven intrinsic regressions. The roughly 100 ms snapshot-write increase merits a controlled follow-up if snapshot throughput becomes a priority; it is outside primary query latency and was not hidden in the totals.

The separate schema-free `entries` experiment uses 4,928 records and no field declarations. Single-run fresh readiness was 46.013 ms Disabled, 52.259 ms ReportOnly, and 53.167 ms AutoIndex; each parsed 4,928 records once. Restores parsed zero records and took 18.703, 18.664, and 22.557 ms respectively. Reopening a Disabled snapshot with AutoIndex rebuilt in 61.391 ms, as required by the mode-specific snapshot contract. Raw output, query/sort/stream counters, first-event and full-page times are in [compact-final](compact-final). This is a one-sample comparison, not a proven speed ordering.

On `entries`, inference reports `complete=false` and admits no paths under the conservative default policy; it therefore does not accelerate this collection. The compact selective title case still scans all 4,928 records because the shared record-level candidate path is not selective enough for the planner. Focused fixtures prove sound source-read reduction for a selective undeclared predicate and exact inferred-posting acceleration on eligible fields. A real Musenalm control on schema-free `places` (210 records, 210 build parses) demonstrates complete inference: 11 paths observed, three admitted, eight dropped, 31,938 estimated inferred-index bytes, eight structural-bitmap bytes. Disabled/ReportOnly/AutoIndex single-run readiness was 5.863/4.949/4.163 ms; do not treat their ordering as a speed claim. See [places-inference](places-inference).

The compact workload covers stored/composite and dynamic sorts, array reduction, cached membership reranking, ordered rows and provisional replacement streams. It does not include declared relation sorts because a schema-free folder has no relation declarations; relation sorting is covered by focused tests and the separately appended registered-schema Q189–Q194 cases. Memory budgets account known retained buffers and key/target state; they are estimates, not hard process RSS limits. The initial source model still requires object records and homogeneous arrays, with unsupported records following the existing diagnosed fallback behavior.

## Reproduction and limits

See the exact commands, versions, schema/corpus fingerprints, profiles and worker/cache settings in the `run-manifest.json` files. `bench/run_optional_features.py` reproduces the compact matrix. `bench/verify_jq_oracle.py` and `bench/verify_extended_musenalm.py` reproduce independent membership checks. The full historical comparison uses the unchanged 150-query workload plus 38 established extension cases; Q189–Q194 extend that manifest separately for the new sorting forms. The compact schema-free workload is separate. The three 17 MiB complete-membership gzip files from the first jq run are preserved locally in `/home/simon/source/objekte-optional-oracle-membership-20261107/`; the final checked per-query hashes and rerunnable oracle are in the repository. No runtime update, automatic relation discovery, or background indexing feature was added.
