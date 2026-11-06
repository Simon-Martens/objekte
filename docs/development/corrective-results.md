# Search-core corrective results

The corrective code revision is `084623a19b` on
`fix/local-file-search-core-compliance`. The final documentation and benchmark
artifact commit measures that code parent. The applicable starting revision is
`8e2e95b746`; finding F was excluded by the assignment.

## Completed findings

| Finding | Status | Fix commits | Focused verification |
|---|---|---|---|
| A: source and field registrations | Complete | `3fb87c6`, `0354298` | A test-only non-JSON reader reaches build, query, payload, and restore; custom/default field bindings agree with independent expected answers and scanner paths |
| B: exact ID and null access | Complete | `225c623`, `162c012` | Folded/raw Unicode ID cases and null-array quantifier matrix with/without postings |
| C: logical types | Complete | `7e11a70` | Required/null/type/temporal/array matrix; invalid numeric strings rejected before indexed and scanned results |
| D: index and query paths | Complete | `6dcfc77`, `941c3b9`, `93ffb1b` | Bounded range-key visits, proof/residual parity, ordered page early exit, exact optional counts |
| E: bounded multi-record scans | Complete | `cf0892a`, `b04b9cb` | Two 2,000-record files, one handle and decode per file/record, dense/sparse/deep pages and payload modes; bounded retained payload and recovered locator throughput |
| G: ownership and configuration | Complete | `4c2bac`, `b47ddcd`, `175493a`, `3479f26`, `084623a` | Immutable state, schema-free queries, discovery/identity, collation/restore compatibility; discovery and symlink tests and final startup measurements |

The detailed step checklist and focused tests are in
[core-implementation-progress.md](../core-implementation-progress.md). All
workspace targets passed on the final code revision. Finding F was deliberately
excluded; no post-read source checks were added.

## Baseline and measurement conditions

The applicable query and build recordings are
[full-5887c7be02](../../bench/results/musenalm/full-5887c7be02/REPORT.md)
and [build-5887c7be02](../../bench/results/musenalm/build-5887c7be02/REPORT.md).
The current [query report](../../bench/results/musenalm/final-084623a19b/REPORT.md),
[600-row comparison](../../bench/results/musenalm/final-084623a19b/comparison.csv),
and [fresh-build report](../../bench/results/musenalm/build-084623a19b/REPORT.md)
retain the complete measurements. The fixture source-metadata and schema-byte
fingerprints match the build baseline exactly; the query corpus fingerprint and
all 150 resolved query definitions match the query baseline. The query runner's
schema and workload hashes differ because their inputs include crate paths that
changed during the crate move. Both runs used rustc 1.99.1 on the same
i9-13900K, 32 query workers, zero source/decoded caches, both bigram alphabets,
disabled and 64 MiB warm verified-result profiles, and five timed repetitions
per query/profile. The OS page cache was not flushed. Query time includes
COUNT, ordering, evidence, and requested page/payload; plans and output writing
were measured separately.

## Correctness and query time

The [independent jq oracle](../../bench/results/musenalm/jq-oracle-084623a19b/REPORT.md)
matched all 150 complete ID sets under each alphabet (300/300). The fixed
workload completed 3,000/3,000 timed requests, with 600/600 plan traces and
zero count/page mismatches across profiles. Every warm request hit the verified
result cache (750/750 per alphabet); disabled profiles had zero verified hits.
The [optimized/reference acceptance logs](../../bench/results/musenalm/acceptance-084623a19b/REPORT.md)
compare counts and full ordered hits, metadata, evidence, and payload under
both alphabets.

The following sums use the 150 per-case medians in each profile, in
milliseconds. Each case's raw five samples and counters are saved in
[measurements.jsonl](../../bench/results/musenalm/final-084623a19b/measurements.jsonl),
with [plans](../../bench/results/musenalm/final-084623a19b/query-plans.jsonl)
and [all printed per-query times](../../bench/results/musenalm/final-084623a19b/console.log).

| Profile | Before, ms | After, ms | Change, ms | Change |
|---|---:|---:|---:|---:|
| ASCII, cache disabled | 2,040.844 | 1,814.125 | -226.719 | -11.1% |
| ASCII, 64 MiB warm | 2.006 | 2.134 | +0.128 | +6.4% |
| Unicode, cache disabled | 1,927.539 | 1,642.518 | -285.021 | -14.8% |
| Unicode, 64 MiB warm | 2.104 | 2.586 | +0.482 | +22.9% |

The warm-profile changes are below 0.5 ms for all 150 queries combined and
should be read as microsecond-scale variation. Four ASCII disabled-cache cases
exceeded both 2 ms and 20% regression thresholds:

| Case | Before, ms | After, ms | Increase, ms | Work comparison |
|---|---:|---:|---:|---|
| Q001 | 13.235 | 26.125 | +12.889 | 4,872 candidates, opens, and parses in both runs |
| Q100 | 16.861 | 21.435 | +4.574 | 22,185 candidates, opens, parses, and residual evaluations in both |
| Q114 | 4.449 | 6.839 | +2.390 | 4,928 candidates, opens, parses, and residual evaluations in both |
| Q115 | 3.805 | 7.746 | +3.941 | 4,928 candidates, opens, parses, and residual evaluations in both |

The bounded scanner's first version slowed scan-heavy locator queries; its
diagnosis led to 256-file batches for payload-free requests, while payload
requests retain bounded worker batches. A later singleton ranking shortcut
did not improve the remaining cases and was removed. The four cases above
still take longer with unchanged source and predicate work. Their residual
latency is an open performance limit; this run does not establish a cause.

## Fresh construction, restore, and memory

The fresh-build series used three separate processes for each alphabet and
snapshot configuration. Every run parsed 148,486 records and passed the
[build regression gate](../../bench/results/musenalm/build-084623a19b/gate.json).

| Configuration | Before, s | After, s | Increase, s | Peak RSS before/after, MiB | Snapshot after, MiB |
|---|---:|---:|---:|---:|---:|
| ASCII, snapshots disabled | 14.222 | 14.549 | +0.327 | 226.8 / 229.2 | — |
| ASCII, snapshot rebuild | 14.530 | 14.848 | +0.317 | 323.6 / 327.8 | 103.4 |
| Unicode, snapshots disabled | 15.447 | 15.741 | +0.294 | 248.3 / 250.6 | — |
| Unicode, snapshot rebuild | 15.840 | 16.139 | +0.299 | 354.4 / 358.8 | 114.1 |

The query runner also launched five fresh restore processes per alphabet.
Median restore-to-ready time was 0.559 to 0.642 s ASCII (+0.083 s) and 0.574
to 0.669 s Unicode (+0.096 s). Snapshot sizes grew from 101.78 to 103.43 MiB
ASCII and 112.47 to 114.12 MiB Unicode. The new discovery rules initially
raised restore to about 1.37 s; the final default-policy path uses directory
entry types and one final sort, avoiding per-file stat and ordered-map work.
The final [index results](../../bench/results/musenalm/final-084623a19b/index-results.json)
retain build, restore, snapshot, and resident-memory samples.

## Focused corrected paths

The reproducible [focused benchmark example](../../crates/core/examples/bench_corrective_cases.rs)
uses temporary JSONL fixtures, four workers, disabled snapshots/caches, and
independent scanner checks outside timing. Its [raw samples and counters](../../bench/results/musenalm/final-084623a19b/focused-cases.jsonl)
and [configuration](../../bench/results/musenalm/final-084623a19b/focused-manifest.json)
are saved.

| Case | Median, ms | Exact result/work observation |
|---|---:|---|
| 10,000-key narrow range | 0.153 | 9 matches, 10 parses; the unit fixture visits only 9 ordered keys |
| Stored-order page without COUNT | 0.140 | 5 hits at offset 7; 12 proven matches processed, count absent |
| Same page with COUNT | 0.291 | Same hits, exact count 10,000; 10,000 proven matches processed |
| Dense two-file JSONL logical page | 2.613 | 4,000 parses, 2 opens, 1,139 peak retained payload bytes |
| Deep offset 1,000 | 3.863 | Same exact count/page, 94,503 peak retained payload bytes |
| Locator / count-only | 2.621 / 2.559 | Zero retained payload bytes in both |
| Sparse two-file JSONL logical page | 0.122 | 8 parses, 2 opens, 840 peak retained payload bytes |

The focused payload counter estimates retained page payload from source record
lengths; it is not process RSS. The fresh-build table reports process peak RSS.
The measurements cover these fixtures and the fixed Musenalm workload, not
arbitrary applications or future query registrations.
