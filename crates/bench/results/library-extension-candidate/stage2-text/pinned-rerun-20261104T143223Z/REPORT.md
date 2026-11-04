# Pinned reference/candidate index-build rerun

The instrumented Musenalm FTS4 build was rerun with **both** binaries pinned to
CPUs `0-15` and `RAYON_NUM_THREADS=16`. Seven independent processes per binary
were run serially in alternating reference/candidate order. No CLI benchmark
was run. The exact commands, exit statuses, stdout JSON and stderr are saved
alongside this report.

Reference production commit: `266d9a0fb071838be8f757671599872a7036a054`.
Candidate commit: `468d97010e393dde130c0102f8f36e952fd2c9e6`. The saved
reference executable has SHA-256
`99e7df67ae59cd739d5c599c173043ee6ef84f101623eb9c12ee76fed68c40e9`.
The candidate was rebuilt from the candidate commit with
`cargo build --locked --release -p objekte-bench --features index-timings --bin index-build-profile`;
its executable has SHA-256
`cac73c4e6049f094289885116cfcaa940d2885da351ecb3ae16b3a8d69532310`.
Both are counting-allocator, stage-timing builds. The original baseline
executable was preserved outside Cargo's target directory.

The benchmark profile is `examples/musenalm`, collection `contents`, ASCII
bigrams, and the four `annotation=markdown`, `preferred_title=plain`,
`title_statement=html`, and `contributors[].annotation=xml` fields. Snapshot
save and restore were included after the timed build. The source content,
filesystem metadata and schema fingerprint matched the Phase 0 baseline
before and after this rerun; see `manifest.json`. Filesystem caches were warm.

| Pair | Reference build (ms) | Candidate build (ms) | Candidate minus reference (ms) |
| ---: | ---: | ---: | ---: |
| 1 | 2757.9 | 2948.6 | +190.7 |
| 2 | 2777.7 | 3046.2 | +268.5 |
| 3 | 2742.2 | 3071.6 | +329.5 |
| 4 | 2883.7 | 3090.1 | +206.3 |
| 5 | 2754.4 | 2827.1 | +72.7 |
| 6 | 2745.6 | 3084.3 | +338.7 |
| 7 | 2747.6 | 2936.6 | +189.1 |

Median build time was **2754.4 ms reference** (range 2742.2–2883.7) and
**3046.2 ms candidate** (range 2827.1–3090.1): **+291.8 ms, +10.6%**.
All seven paired differences were positive. Every process reported 16 Rayon
workers, 133,948 documents, identical index component sizes, 47,456,174
compressed index bytes, and identical base/secondary snapshot sizes
(19,511,794/66,599,600 bytes). Median allocation counts were 27,092,950
reference and 27,093,776 candidate; median allocated bytes were 5,007,470,520
and 5,023,634,117 (+0.32%).

The slower candidate is still reproducible when both binaries have identical
CPU affinity and thread counts. The profiler's summed worker stage times were
also higher, including source-read and JSON-parse time, but this instrumented
run alone does not establish a causal code path or a normal-release slowdown.
The performance gate remains open. The earlier normal-release CLI control is
not part of this rerun.
