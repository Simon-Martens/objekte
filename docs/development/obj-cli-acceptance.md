# `obj` CLI implementation and acceptance

## Revisions and result

The prerequisite development revision was `4a93382f0404ae4cc6d1e7c028b7a48cacd006f4`. The final measured implementation is `45084cdbc3c7041b204c1771c5d18b9c7bb14092` on `integration/obj-cli`. The older Musenalm comparison recording measures `ef44e42e566abe1a808094cd7933cc798a63f63d`. The final documentation commit and development promotion do not alter the measured code.

The workspace now builds `crates/cli` as `objekte-cli`, producing `obj`. It opens one directory as a schema-free `files` collection, admits any JSON root and mixed arrays, retains exceptional JSON numbers, supports `@root` paths, searches decoded scalar values, streams complete JSON documents as ordered NDJSON, and loads or writes snapshots only when requested. Public path IDs remain stable. The default is all results, one execution per query. The CLI accepts direct structured/FTS queries and line-based batches. [CLI usage and event contract](../cli/obj.md) gives the exact syntax and limits.

The fresh decoded-scalar reader now constructs its logical value and source-order scalar text in one structural traversal. Scalar strings are decoded once and temporarily cloned into text segments for indexing; the repository does not retain a second full decoded corpus. This change preserved the measured snapshot byte sizes for the 4,928-document `entries` folder. Query-time selected scalar FTS still performs a second walk of already validated source bytes to retain lexical number tokens and duplicate-name semantics; it is a possible CPU cost for broad scans, and does not count as a second source read or reader parse.

## Correctness gates

| Check on final code | Result |
| --- | ---: |
| `cargo test --workspace --all-targets` | 357 passed, 0 failed, 7 ignored |
| `cargo clippy --workspace --all-targets` | Exit 0; pre-existing advisory warnings remain |
| Independent schema-free scalar/root oracle, ASCII and Unicode | 44/44 complete ID sets |
| Handwritten jq oracle over 148,486 original Musenalm JSON files | 300/300 complete ID sets |
| Independent raw-JSON relation oracle | 18/18 complete ID sets |
| Optimized versus reference Musenalm Q001–Q194, both alphabets | 388/388 cases; counts, ordered pages, payloads and evidence agree |
| Final release workload, four profiles × five repetitions × 194 cases | 3,880/3,880 successful samples |

The [scalar oracle log](../../bench/results/obj/cli-integration-20261107/scalar-oracle-final.log), [jq manifest](../../bench/results/obj/cli-integration-20261107/jq-oracle-final/run-manifest.json), [relation manifest](../../bench/results/obj/cli-integration-20261107/relation-oracle-final/run-manifest.json), and [ASCII](../../bench/results/obj/cli-integration-20261107/accept-ascii-45084cd.log)/[Unicode](../../bench/results/obj/cli-integration-20261107/accept-unicode-45084cd.log) acceptance logs are saved. Process tests also cover snapshots, malformed fallback, root values, query files, hidden/snapshot exclusions, all results over 50 records, output errors, cancellation and broken pipes. Snapshot/model versions are 13/9, so old scalar-text representations cannot be restored as current indexes.

The repository-wide `cargo fmt --all -- --check` and the spacing check report inherited formatting drift across unrelated core/TUI files. The new CLI crate and changed scalar-text files were formatted using the repository conventions, and `git diff --check` passes. Clippy with `-D warnings` fails on existing advisory warnings; the prescribed normal Clippy command passes.

## Release measurements

Measurements ran sequentially on the same Linux machine with Rust 1.99.1 release binaries, CPU governor `performance`, affinity CPUs 0–31, and no filesystem cache dropping. Repeated process runs are filesystem-warm or unspecified, not cold-cache measurements. The Musenalm baseline and final run have identical content fingerprints, schema and 188 shared query definitions, 32 query workers, zero source/decoded caches, ASCII and Unicode, verified cache off or 64 MiB warm, and five timed repetitions. Six sort queries were appended to the final run. The fixture is an isolated hard-linked copy; metadata and OS cache history can still affect wall time. Values are sums of per-query medians, not elapsed time for a batch. The [environment record](../../bench/results/obj/cli-integration-20261107/environment.json) captures the machine and build settings.

| Shared queries | Profile | Baseline ms | Final ms | Difference ms | Difference % |
| --- | --- | ---: | ---: | ---: | ---: |
| Q001–Q150 | ASCII, cache off | 1,909.970 | 1,606.128 | -303.842 | -15.91% |
| Q001–Q150 | Unicode, cache off | 1,950.214 | 1,568.365 | -381.849 | -19.58% |
| Q151–Q188 | ASCII, cache off | 392.330 | 333.346 | -58.985 | -15.03% |
| Q151–Q188 | Unicode, cache off | 420.775 | 324.624 | -96.151 | -22.85% |
| Q001–Q150 | ASCII, warm | 1.454 | 1.329 | -0.125 | -8.61% |
| Q001–Q150 | Unicode, warm | 1.450 | 1.379 | -0.071 | -4.93% |

For all 188 shared cases, result counts, returned page sizes, candidates, file opens, scans and parses match the prior recording. Q189–Q194 are new sort cases with no older equivalent in this recording. The [776-row query table](../../bench/results/obj/cli-integration-20261107/FULL_QUERY_COMPARISON.csv) contains each full request, medians, ranges, absolute/percentage differences and work counters. [Raw timed samples, plans, queries, build/restore data and environment](../../bench/results/obj/cli-integration-20261107/musenalm-timed-45084cd/) are retained.

One broad metadata query, Q119 (`@file.modified >= '2026-01-01T00:00:00Z'`), varied materially between full runs despite identical 133,948 exact matches and zero source opens/scans/parses. Alternating 20-sample ASCII blocks measured old 7.17/7.06 ms, pre-fix 8.11/8.02 ms, and the allocation fix 7.54/7.56 ms. The final full-run ASCII median is 8.10 ms. Its Unicode full-run median was 11.74 ms versus an 8.54 ms older recording, but a later paired block measured old 12.37 ms and fixed 7.49 ms. These contradictory process timings and identical work counters do not establish a remaining intrinsic regression. The scoped fix removes a temporary single-value vector in scalar metadata evaluation; its focused null/timestamp test and the full acceptance suite pass. [Paired raw results](../../bench/results/obj/cli-integration-20261107/q119-paired/) retain the samples.

Registered-schema Musenalm construction parsed 148,486 records once in both builds and retained 15 bigram stores with unchanged gram counts and snapshot bytes. ASCII build was 1,232.223 → 1,168.625 ms and Unicode 1,284.726 → 1,224.046 ms; five-sample restore medians were 546.541 → 534.395 ms and 559.823 → 526.835 ms. The total construction/snapshot differences include process and cache variation and are not attributed solely to CLI changes. The final ASCII/Unicode snapshot sizes are 117,262,305/119,772,881 bytes.

### Schema-free `obj` measurements

The deterministic [fixture generator](../../bench/obj/generate_fixtures.py) created a 13-admitted-document general folder plus one malformed fallback and a separate 1,024-document export folder with 2 KiB padding per document. The [measurement driver](../../bench/obj/measure_cli.py) ran five sequential release processes per scenario, captured NDJSON terminals, process wall time and peak RSS, and saved [all raw samples](../../bench/results/obj/cli-integration-20261107/cli-measurements-45084cd/measurements.json). The table gives medians on final code; query time excludes startup but includes complete-document output callbacks.

| Scenario | ASCII process / query ms | Unicode process / query ms | Relevant result |
| --- | ---: | ---: | --- |
| General fresh build + snapshot write | 6.056 / — | 5.969 / — | 13 successful parses, one malformed fallback |
| General restore | 2.185 / — | 2.154 / — | 0 build parses |
| Selective FTS `Goethe` | 2.371 / 0.080 | 2.402 / 0.084 | 1 hit, 1 scan/parse |
| Dynamic `title ~ "Goethe"` | 2.423 / 0.099 | 2.254 / 0.080 | 2 hits under existing empty-array semantics |
| Numeric `year >= 1800` | 2.483 / 0.095 | 2.030 / 0.090 | 13 scans/parses; 2 hits |
| Finite computed sort | 2.505 / 0.321 | 2.654 / 0.310 | 2 hits; sort barrier before first hit |
| 1,024-document fresh build + write | 32.676 / — | 46.150 / — | 1,024 build parses |
| 1,024-document restore | 4.267 / — | 4.244 / — | 0 build parses |
| All-results FTS export | 13.219 / 8.481 | 15.333 / 9.953 | 1,024 hits, 1,024 scans/parses; first hit 0.166/0.248 ms |
| Computed sort, all results | 32.528 / 27.823 | 25.700 / 20.109 | 1,024 hits; first hit 20.083/14.907 ms |
| Computed sort, 50-row page | 18.304 / 12.872 | 17.981 / 13.331 | 50 emitted, exact total 1,024 |

The ordinary all-results export opened 1,024 source files and parsed each once for verification and payload. The all-results computed sort opened 2,048 files: one bounded pass for values/keys and one later payload read in final order. It did not retain all document bodies; the measured process peak was about 7.4–7.6 MiB for computed sort and 7.0 MiB for ordinary export. These are process RSS observations for this fixture, not hard limits. The core has explicit event, buffered-result and computed-sort budgets as documented in the CLI guide. The [snapshot-size manifest](../../bench/results/obj/cli-integration-20261107/snapshot-sizes.json) records generated snapshot sizes; benchmark snapshot payloads were removed after measurement.

On the real 4,928-document schema-free `entries` folder, final five-process median fresh startup with snapshot write was 72.359 ms ASCII and 80.468 ms Unicode; restore startup was 18.947/18.537 ms and reparsed zero source records. Each fresh build parsed exactly 4,928 records. These are [saved separately](../../bench/results/obj/cli-integration-20261107/entries-45084cd-measurements.json). They cannot be compared as an unchanged-text-policy speedup with the earlier string-only `entries` benchmark: scalar values now contribute text and this invocation writes a snapshot.

## Limits

Discovery selects case-sensitive `.json` regular files, skips nested symlinks, and prunes `.git` and the selected snapshot directory. Deep or exceptionally large JSON and computed sorts can return explicit resource errors. A broad computed sort must evaluate keys before the first row. The CLI does not infer a schema, discover relations automatically, watch source changes or provide an external sort engine. Snapshot freshness follows the existing metadata reconciliation contract. The query-time duplicate/lexical scalar-text walk remains a possible optimization target; no separate source read is introduced by it.
