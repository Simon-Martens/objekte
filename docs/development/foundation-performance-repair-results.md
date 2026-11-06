# Core foundation performance repair results

Measured code parent: `20261604bebf949131441c8262f1f442bce4207c` on
`fix/core-foundation-performance`. The immediate pre-repair core is
`92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. All repair commits were
kept separate and pushed. This report and its benchmark artifacts are the
documentation commit; no core code changed after the measured parent.

## Acceptance status

**Correctness passed.** The final code passed 90 core tests, 25 TUI tests,
and the CLI test. The 150-case acceptance harness compared complete ordered
hits, count and page results against the reference path for both alphabets,
including status, record class, diagnostic reference, offsets, evidence and
payload. Both restored snapshots passed. The independent handwritten `jq`
oracle matched all 300 complete ID sets (150 cases × two alphabets) over
148,486 original JSON files. The small mixed-source tests independently
check unavailable truth values, diagnostic association, payload states,
custom scanner callbacks, sorted pages, cache and damaged snapshots. See the
final [oracle report](../../bench/results/musenalm/repair-jq-oracle-20261604be/REPORT.md),
[acceptance logs](../../bench/results/musenalm/controlled-repair-final-20261604be/),
and [workspace test log](../../bench/results/musenalm/controlled-repair-final-20261604be/workspace-tests.log).

**The raw per-case query gate did not pass.** Its first policy-matched 3,000
sample run had nine timing failures; the justified repeat had nine, with only
Q052 and Q114 in common. Counts, pages and sample status passed in both.
Controlled A–B–B–A checks did not confirm a slowdown beyond the unchanged
gate's 1.5× **and** 2 ms per-case allowance. The performance gate remains
open because its full-run failures are real measurements, even though their
changing membership and overlapping paired distributions indicate substantial
run variation. This is a limit on the performance conclusion, not a change to
the gate threshold.

## Comparable measurement conditions

The original foundation recordings
[`final-92cdb92b19`](../../bench/results/musenalm/final-92cdb92b19/) and
[`build-92cdb92b19`](../../bench/results/musenalm/build-92cdb92b19/) used
the `powersave` governor. During final measurement the host reported
`performance`. No machine-wide policy was changed for this work. A complete
new **before-state** was therefore recorded using the unmodified foundation
binary under the current policy: [query baseline](../../bench/results/musenalm/controlled-foundation-performance-92cdb92b19/)
and [build baseline](../../bench/results/musenalm/controlled-foundation-build-performance-92cdb92b19/).
The matching final runs are [queries](../../bench/results/musenalm/controlled-repair-final-20261604be/),
[query repeat](../../bench/results/musenalm/controlled-repair-final-repeat-20261604be/),
and [builds](../../bench/results/musenalm/controlled-repair-build-20261604be/).

All controlled runs used the i9-13900K, affinity `0-31`, Rust 1.99.1,
release builds with no extra build flags, the same corpus, 32 query workers,
and zero source/decoded-cache budgets. Each query run kept the established
150 cases, ASCII/Unicode indexes, verified cache disabled or warmed at 64 MiB,
five timed repetitions per case/profile, and count/page/payload work inside
timing. The OS file cache was warm; “uncached” below refers to the verified
result cache. Plans and analysis were captured outside timed samples. The
runner's old schema and workload hashes include absolute checkout paths, so
the gate also compares parsed schema content and resolved query JSON when
worktrees differ. The corpus fingerprint and semantic registrations agree.
The [full 600-row timing table](../../bench/results/musenalm/controlled-repair-final-20261604be/TIMING_TABLE.md),
[repeat table](../../bench/results/musenalm/controlled-repair-final-repeat-20261604be/TIMING_TABLE.md),
raw samples, plans, counters, commands and manifests are saved beside them.

## Query latency

These totals sum 150 per-query medians per profile; they are not a single
workload wall time. The first policy-matched final run measured 3,000 valid
samples and an aggregate ratio of 0.994; the repeat measured 1.012.

| Profile | Foundation ms | Final ms | Difference ms | Difference % |
| --- | ---: | ---: | ---: | ---: |
| ASCII, verified cache off | 2,075.7 | 2,054.3 | -21.4 | -1.0% |
| Unicode, verified cache off | 1,873.0 | 1,871.6 | -1.4 | -0.1% |
| ASCII, warm verified cache | 1.7 | 1.8 | +0.1 | +5.9% |
| Unicode, warm verified cache | 2.4 | 2.7 | +0.3 | +12.5% |

Broad uncached scans have effectively unchanged work counts. For example,
Q006 parses and opens 148,486 records/files in both versions and measured
232.744→230.192 ms in the first controlled run. Q025 opens/parses 67,748
and measured 117.874→122.676 ms; its paired blocks were 117.1→121.8 ms
for ASCII and 122.9→119.4 ms for Unicode. Q077 measured 121.370→99.610 ms
in the first full run but 116.145 ms on repeat. Selective Q099 has zero
source opens/parses in both versions; its 10.123→2.967 ms full-run change is
not attributable to a source repair, and paired blocks varied widely.
Warm-cache totals are in single-digit milliseconds, so their percentages
correspond to only tenths of a millisecond.

The first controlled gate flagged Q022 ASCII, Q052 ASCII, Q068 ASCII, Q069
both alphabets, Q096 Unicode, Q099 Unicode, Q100 ASCII and Q114 ASCII. The
repeat flagged Q027 ASCII, Q052 ASCII, Q055 ASCII, Q072 ASCII, Q074 Unicode,
Q096 ASCII, Q114 ASCII, Q115 ASCII and Q144 ASCII. Q052 ASCII was 5.220 ms
at baseline, then 13.329 and 8.484 ms; paired foundation blocks were 5.311
and 11.516 ms, versus repair blocks 7.605 and 10.481 ms. Q114 ASCII was
3.560 ms at baseline, then 5.672 and 6.633 ms; a later paired check measured
foundation blocks 4.912 and 5.176 ms versus repair blocks 3.989 and 3.533
ms. These distributions do not support a repeatable >2 ms regression in
either query. The two paired studies retain all raw 11-repetition samples,
run order, exact commands and counters:
[first](../../bench/results/musenalm/repair-final-failures-paired/) and
[policy-matched](../../bench/results/musenalm/repair-controlled-failures-paired/).
They do not make the full gate green.

The earlier corrected-core reference `084623a19b` is a separate comparison:
its original `powersave` recording summed 3,461.4 ms, versus 4,585.2 ms for
the pre-repair foundation `92cdb92b19` under that policy (+32.5%). The
current `performance`-governor totals must not be used to claim a percentage
recovery against that earlier reference.

## Fresh builds, restore and memory

Each build configuration ran in three separate processes. Every run parsed
148,486 records successfully and had zero failed parses. Snapshot sizes were
unchanged: 111,176,829 bytes for ASCII and 122,383,021 for Unicode.

| Configuration | Foundation wall s | Final wall s | Difference s | Final peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| ASCII, snapshots disabled | 11.142 | 11.343 | +0.201 | 236.4 |
| ASCII, snapshot rebuild | 11.359 | 11.842 | +0.483 | 339.4 |
| Unicode, snapshots disabled | 12.185 | 12.089 | -0.096 | 257.8 |
| Unicode, snapshot rebuild | 12.482 | 12.647 | +0.165 | 370.4 |

The final build's coarse phases show approximately 0.26 s discovery,
10.96–11.96 s extraction/indexing, and 0.12 s finalization. The healthy
corpus uses about 57.1 MiB estimated inventory heap and zero diagnostic
heap; these are separate from index estimates and process RSS. The full
[build report](../../bench/results/musenalm/controlled-repair-build-20261604be/REPORT.md)
and raw samples include resident and peak memory for every configuration.
The differences above are small relative to run variation; no construction
speedup is claimed.

Five fresh-process restores per alphabet were measured by the established
query runner. Each restored all 148,486 records with zero build parses.

| Alphabet | Foundation wall ms | Final wall ms | Difference ms | Decode/validation ms, before→after | Reconciliation ms, before→after |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | 602.0 | 494.6 | -107.4 | 356.5→254.4 | 202.9→202.9 |
| Unicode | 685.8 | 512.7 | -173.1 | 440.0→268.4 | 205.8→204.1 |

Restore median resident memory was about 226 MiB ASCII and 240 MiB Unicode;
median peak memory about 234–235 and 253–254 MiB respectively, similar
before and after. The phase timer includes snapshot read, decode and
validation, so it cannot assign the entire gain solely to bitmap traversal.

## Repair attribution and commits

| Repair | Commit and focused evidence | Removed work and result |
| --- | --- | --- |
| Positioned source reads | `c5787c1d80`; [focused report](../../bench/results/musenalm/repair-seek-focused/REPORT.md) and range-reader tests | Avoids a redundant seek for a fresh zero-offset read and adjacent reads on reused handles. Q006 232.054→227.240 ms in focused blocks; Q025 104.234→94.094 ms, but its baseline blocks varied. No precise broad-scan gain is established. |
| Distinct file accounting | `ce9d12260a`; [focused report](../../bench/results/musenalm/repair-accounting-focused/REPORT.md) and shared-file/payload tests | Removes the extra hash-set pass after file grouping; compact integer union handles overlapping collections and payloads. Q006 241.567→239.515 ms, smaller than block variation. Attribution remains unresolved. |
| Construction clone | `d3f24b8c81`; [focused report](../../bench/results/musenalm/repair-file-clone-focused/REPORT.md) and malformed-source tests | Removes one full `FileEntry` clone per selected source file while preserving streaming diagnostics. ASCII disabled build 11.722→11.708 s; no measurable speedup. |
| Validation | `68ad3c380d`; [focused report](../../bench/results/musenalm/repair-validation-focused/REPORT.md) and corruption tests | Removes redundant all-logical subset traversals and the second diagnostics validation, while checking classifications and serialized eligibility. Focused restore decode/validation fell 392.901→302.016 ms ASCII and 449.398→313.944 ms Unicode. The final matched-policy restore result is above. |
| Mixed-domain planner | `dbf4777adf`; [focused report](../../bench/results/musenalm/repair-mixed-planner-focused/REPORT.md) and independent truth fixtures | A malformed fallback no longer disables selective logical postings for healthy slots. The focused mixed query went 2.311→0.106 ms: 10,002→1 candidates, 10,001→0 parses, 1→0 opens. Healthy control was 0.099→0.104 ms. Snapshot version advanced to 6 with corruption coverage. |
| Acceptance contract | `af18fdcd4c` | Full `SearchHit` equality and first-field diagnostics; independent malformed/healthy answers, stable resolved diagnostics across restore, and a partial custom scanner with a fallback slot. |
| Benchmark identity | `9b8aa71317`, `20261604be` | Explicit baseline SHA and comparison type, semantic cross-worktree compatibility, governor/affinity checks, full timing tables and raw gate failures. |

Clippy completed successfully at the measured parent. It reported 47 existing
core library warnings plus two benchmark-example warnings; no new warning
was introduced by these repairs. Formatting and the repository's Rust spacing
tool were run before the final code commits. The source repair and acceptance
commits preserve bounded source handles, grouped JSONL reads, early stored-order
termination, cache behavior, and corruption rejection.

The remaining uncertainty is short-query and index-only timing variation on
this shared host. The raw full-suite gate remains red despite correctness,
structural work reductions and improved restore. A future performance
acceptance run should keep the declared baseline policy, run when host load is
stable, and compare the retained raw samples and counters before changing
query code. The guide's per-case and aggregate thresholds were not altered.
