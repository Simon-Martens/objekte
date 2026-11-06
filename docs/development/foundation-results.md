# Foundation correction: final verification

Measured code parent: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b` on
`fix/core-foundation-correctness`. This report and its evidence are a separate
documentation commit. The applicable corrected-core before state is
`084623a19bf26fc56b470b6548c38382dff4d880`; the older `5887c7be02`
measurements are historical context. The corpus has 148,486 JSON records.

## Implemented corrections

| Requirement | Code commit | Verification |
|---|---|---|
| Compatible and complete bigram reuse for raw, field and named bindings | `0cfa738446`, `6a5f4d61e1` | Independent match IDs, scanner parity, rejected-path plans, build/restore; Q070 uses one candidate after the named-transform correction |
| Optional temporal ancestors and projected conversion | `091b0cc701` | Absent/present/null parents, arrays, payload modes, scan/index, restore |
| Nested array length quantifiers and exactness | `23006cf256` | Multiple reached arrays, every supported comparison and quantifier, negation, reordered elements |
| Prepared custom exact binding | `64c0dd5ef7` | Custom separator binding, callback and preparation counters, posting/bigram/scan paths and restore |
| Reader-owned extracted text | `58883ba9e5` | Excluded fields and segment boundaries with temporal declarations, build/query/restore |
| Shared record preparation and bounded source access | `522bf60106`, `093b7c0223` | Ordered residual pages, direct hits, decode reuse, cache paths, open/read/retained-byte counters |
| Framing, source error boundaries, status and diagnostics | `ed48397ac2`, `daa031fe19` | Invalid ranges, recoverable versus fatal errors, stable status codes, bounded diagnostics and serialization |
| Malformed JSON/JSONL retention and raw verification | `fc9dc1b36a` | Raw fallback IDs/ranges, unavailable logical truth, evidence, payloads, cache, CLI and restore |
| Typed posting values and bounded exact counts | `741e95a056`, `dd8dd5e5ec` | Distinct-record counts, typed order, prefix/range/pagination, exact selections, coverage and zero-source-read tests |
| Cross-feature and failure invariants | `a097a76867` | Differential truth tables, seeded candidate conservatism, source boundaries and snapshot corruption |
| Benchmark instrumentation and focused cases | `3f1635b623` | Separate successful parses and raw verifications, custom scanner, malformed and posting-value measurements |
| Off-page evidence allocation | `92cdb92b19` | Exact posting proof with and without membership cache; Q099 focused before/after |

All changes were separately committed and pushed in this order. The formatting
convention was applied in its own commit (`5531f6d0ba`). The core API still
supports in-memory schema construction; file-based schema loading remains an
adapter.

## Correctness checks

- `cargo test --workspace --quiet`: 84 core tests, 24 TUI tests and two further
  test targets passed; no failures.
- `python3 scripts/format-rust-spacing.py --check` and `git diff --check`
  passed. `cargo clippy --workspace --all-targets` exited successfully with
  existing advisory warnings (47 on the core library); it was not a
  zero-warning run.
- The [jq oracle](../../bench/results/musenalm/jq-oracle-92cdb92b19/REPORT.md)
  independently evaluated handwritten predicates over the original files.
  Complete membership ID sets matched in 300/300 case/alphabet comparisons.
- The [optimized versus index-free acceptance](../../bench/results/musenalm/acceptance-92cdb92b19/REPORT.md)
  passed 150/150 cases in each alphabet after snapshot restore. It compares
  counts, ordered pages, evidence, file metadata, ranges and payloads. Its
  harness does not compare the new status/classification/diagnostic fields;
  focused tests check those routes.
- The [first final run](../../bench/results/musenalm/final-92cdb92b19/REPORT.md)
  and [repeat run](../../bench/results/musenalm/final-repeat-92cdb92b19/REPORT.md)
  each recorded 3,000 successful timed query samples: 150 cases, two alphabets,
  disabled and 64 MiB verified-cache profiles, five repetitions. Each saved
  600 per-query rows, 600 plans/counters, raw samples, resolved queries,
  environment and configuration, correctness checks and the full
  [per-query timing table](../../bench/results/musenalm/final-92cdb92b19/per-query-timings.md).
  Every reported count matched jq; page sizes matched the applicable before
  state. All 1,500 warm samples per run hit the verified cache. Source and
  decoded cache budgets were zero, with 32 query workers.
- Deterministic cross-feature tests cover build/restore, Boolean unavailable
  truth, missing/null/broken distinctions, status in cache and output, raw
  bytes, diagnostic references, custom binding and source reader boundaries.
  `cargo llvm-cov` is not installed, so no coverage percentage is claimed.

The original fixed corpus, schema and resolved 150 queries agree with the
`084623a19b` before recording. Registration indexed-path order is compared as
a set because its source map is unordered. The older pinned gate's path-based
schema/workload hashes differ, although the current and applicable before
fingerprints and resolved query content agree. Those two gate flags do not
indicate answer drift.

## Query performance

The fixed 150-query protocol includes construction and fresh-process
restore-to-ready measurements. Query timings include count and requested
page/payload; diagnostic plans are captured outside timed samples. The sums
below add the 150 per-case medians for each profile; they are comparison
summaries, not a single whole-workload elapsed time.

| Profile | Before `084623a19b` | First final | Repeat final |
|---|---:|---:|---:|
| ASCII, verified cache off | 1,814.125 ms | 2,340.814 ms (+29.0%) | 2,045.345 ms (+12.7%) |
| Unicode, verified cache off | 1,642.518 ms | 2,239.086 ms (+36.3%) | 1,817.716 ms (+10.7%) |
| ASCII, verified cache warm | 2.134 ms | 2.201 ms (+0.067 ms) | 1.794 ms (-0.340 ms) |
| Unicode, verified cache warm | 2.586 ms | 3.086 ms (+0.500 ms) | 2.715 ms (+0.129 ms) |

The query latency gate **does not pass**. Beyond its two older path-hash flags,
the first run flags Q014 Unicode, Q099 Unicode and Q148 in both alphabets; the
repeat flags Q099 ASCII, Q141 ASCII and Q148 ASCII. Complete results and
counter comparisons are in each run's `comparison.csv` and
`per-query-timings.md`. The large run-to-run variation, including Q099 moving
from 10.722 to 18.502 ms in ASCII and 14.712 to 2.879 ms in Unicode, means
small percentage changes here should not be treated as stable improvements.

A separate [controlled 11-repetition subset](../../bench/results/musenalm/controlled-new-92cdb92b19/COMPARISON.md)
ran the before and after cores alone in separate processes against the same
physical corpus and schema. Resolved query JSON was byte-equivalent. Q006
was 227.346→244.480 ms ASCII (+7.5%) and 228.256→242.509 ms Unicode (+6.2%);
Q007 was 286.124→280.346 ms ASCII (-2.0%) and 269.593→290.803 ms Unicode
(+7.9%). Q099 ASCII improved 8.300→2.573 ms (-69.0%), but Unicode measured
8.482→10.066 ms (+18.7%). Q148 ASCII remains material at 4.950→8.065 ms
(+62.9%); Unicode measured 6.795→5.826 ms (-14.3%). Candidate and parse
counts for the investigated broad scans are unchanged. Preparation/planning
is a small fraction; source scanning, parsing and machine/scheduling variation
dominate. The cause of the Q148 ASCII difference and the higher uncached
aggregate is not isolated. The machine used the `powersave` governor and had
other desktop load, with no CPU affinity or OS cache flush in this established
protocol.

The earlier historical Q001/Q100/Q114/Q115 regressions versus `5887c7be02`
are not resolved simply by the aggregate changes. Q001 ASCII measured
13.235 ms historically, 26.125 ms at the applicable before state, and
14.812/11.753 ms in the two final runs. Q100 was 16.861, 21.435 and
21.611/16.473 ms; Q114 was 4.449, 6.839 and 4.855/6.405 ms; Q115 was
3.805, 7.746 and 6.503/6.036 ms. Their candidate/open/parse counts are
unchanged between `084623a19b` and final. These measurements do not establish
a single new overhead source; Q115 in particular remains slower than its
historical recording.

## Build, restore and focused resource measurements

The [fresh build series](../../bench/results/musenalm/build-92cdb92b19/REPORT.md)
used three separate processes per configuration. Every build parsed 148,486
records and passed the existing build gate. Medians versus `084623a19b`:

| Configuration | Before wall | Final wall | Final peak RSS | Snapshot |
|---|---:|---:|---:|---:|
| ASCII, snapshot disabled | 14.549 s | 16.861 s (+15.9%) | 236.2 MiB | — |
| ASCII, rebuild snapshot | 14.848 s | 17.319 s (+16.6%) | 339.5 MiB | 106.0 MiB |
| Unicode, snapshot disabled | 15.741 s | 17.916 s (+13.8%) | 257.7 MiB | — |
| Unicode, rebuild snapshot | 16.139 s | 18.316 s (+13.5%) | 370.0 MiB | 116.7 MiB |

The query runner's five fresh-process restore-to-ready samples had median
0.781 s ASCII and 0.890 s Unicode in the first run versus 0.642 s and
0.669 s before. The repeat measured 0.606 s and 0.691 s, illustrating
environment sensitivity. Snapshot sizes grew from 103.4 to 106.0 MiB ASCII
and 114.1 to 116.7 MiB Unicode. Peak process RSS is reported separately
from the engine's estimated resident index bytes.

The [focused measurements](../../bench/results/musenalm/focused-92cdb92b19/manifest.json)
are first measurements for new scenarios, with raw samples in
`measurements.jsonl`:

| Scenario | Median | Resource observation |
|---|---:|---|
| Page-only stored-order JSONL, offset 1,000, limit 5 | 0.490 ms | 1,005 parses and residual evaluations; one source open; no count published |
| Same JSONL with count requested | 3.648 ms | 10,000 parses and evaluations; one source open |
| Direct exact logical hit | 0.106 ms | One parse, one source open |
| Custom posting, bigram, scanner | 0.029 / 0.150 / 0.261 ms | 0 / 500 / 1,000 parses; 0 / 1 / 1 opens; callbacks measured |
| Incompatible custom raw representation | 0.297 ms | Conservative scan of 1,000 records, one open |
| Mixed malformed raw FTS | 0.451 ms | 390 successful decodes, 10 raw verifications, one open |
| Healthy versus 2.5% malformed JSONL build | 18.041 / 18.953 ms | 400 successful parses versus 390 successful plus 10 failed attempts; one 1 MiB malformed record |

High-cardinality posting-value prefix, range and exact-selected pages took
roughly 0.001 ms each in this small fixture, visited only bounded dictionary
keys (11 for the narrow pages), and performed zero source reads. These times
are too small to generalize from; the key/read counters are the stable check.
Custom and mixed restore rows are also saved. The earlier focused Q070 path
regression improved from 5.365/5.349 ms and 4,010 parses to 0.166/0.131 ms
and one parse in the corresponding ASCII/Unicode runs after `6a5f4d61e1`.
Deferring off-page exact-proof evidence in `92cdb92b19` reduced focused Q099
ASCII from 8.300 to 2.573 ms in the controlled subset, while its Unicode
timing remained variable.

## Limits

The new malformed-source semantics and posting-value API are covered by
focused fixtures, including restore; the fixed Musenalm corpus contains no
malformed records, so the jq oracle does not exercise fallback truth or status.
The broad 150-query benchmark did not meet its latency gate, and fresh builds
were 13.5–16.6% slower than the applicable before state. These are remaining
performance issues, not correctness passes. The acceptance example omits
status/classification/diagnostic comparison, and `cargo llvm-cov` is not
available for coverage measurement.
