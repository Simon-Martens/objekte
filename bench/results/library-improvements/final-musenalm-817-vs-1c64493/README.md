# Matched Musenalm library acceptance

Baseline code `81788174898c27b2c2c367319bad20ecee450562` and corrected code `1c64493ab981790de606def56c2b11e0dea1dd3a` ran sequentially on the same mtime-preserving copy of `examples/musenalm`. The final example binary was compiled at `1c64493`; its run manifest reports later Git HEAD `98410748d0` because that commit contains only independent-oracle documentation. Binary SHA-256 values, exact commands, compiler, CPU, cache settings and fingerprints are in [environment.json](environment.json).

Both examples loaded the same resolved 194 queries and schema manifest byte for byte. The raw `workload_fingerprint` differs because the example hashes the absolute manifest path as well as file contents; the manifest bytes have the same SHA-256 in both worktrees. Corpus/schema fingerprints and registration definitions match. Each process ran ASCII and Unicode with verified cache off and 64 MiB warm, five timed repetitions per case, 32 query workers, zero source/decoded cache bytes, and a fresh build with snapshots disabled. Snapshot save and five fresh-process restore samples were recorded separately from timed queries. The two benchmark-created snapshots in the copied fixture were removed between processes.

Regenerate the comparison table with `python3 bench/compare_query_extensions.py BASELINE_DIR FINAL_DIR COMPARISON_DIR --environment bench/results/library-improvements/final-musenalm-817-vs-1c64493/environment.json`; the recorded `BASELINE_DIR`, `FINAL_DIR` and `COMPARISON_DIR` are the sibling directories of this file.

All 776 case/profile rows completed without error. Answers, page sizes, candidates, source opens, bytes read, parses, exact-posting matches, scans and residual-evaluation counts match between revisions for every row. The independent jq oracle passed 300/300 original cases at the corrected SHA; the independent relation oracle passed 18/18. [Complete ID-set comparison](extended-ids/run-manifest.json) of the other 44 extended cases passed 88/88 across alphabets. The ID dumps compare membership, while focused sort tests and benchmark page/count checks cover ordering behavior. All [raw query samples](baseline/measurements.jsonl), [final samples](final/measurements.jsonl), plans, manifests and build/restore recordings are preserved. The [complete 776-row comparison](comparison/COMPARISON.md) prints each resolved query and its options, median time in milliseconds to three decimals, source/scan work, total matches and page size; [CSV](comparison/comparison.csv) retains raw median nanoseconds too.

## Query timing

These are sums of 194 *per-query medians*, not process wall time. Each median comes from five timed repetitions in one process per revision. Delta is corrected minus baseline.

| Profile | Baseline ms | Corrected ms | Delta ms |
| --- | ---: | ---: | ---: |
| ASCII, verified cache off | 5765.602 | 4010.373 | -1755.229 |
| Unicode, verified cache off | 5641.833 | 4067.079 | -1574.754 |
| ASCII, 64 MiB warm | 1603.920 | 1611.146 | +7.225 |
| Unicode, 64 MiB warm | 1612.210 | 1618.199 | +5.989 |

The largest uncached gains are computed-sort cases Q191 (ASCII 1573.118→548.450 ms; Unicode 1545.774→570.060 ms) and Q192 (ASCII 1070.762→379.344 ms; Unicode 1078.157→425.622 ms). Q190 also improves 79.807→18.791 ms in ASCII and 73.342→19.574 ms in Unicode. Their source-work counters are unchanged; the improvement is in sort/execution work. Q191/Q192 warm-cache times remain about 923–929 and 626–635 ms respectively, slower than their new uncached routes. This warm-versus-uncached gap predates the change and is a remaining performance opportunity. Q192's warm baseline-to-final medians rise by 8.021 ms in ASCII and 5.335 ms in Unicode in these single-process recordings; those small changes are inconclusive.

Some broad scans showed slower five-sample medians despite identical work: Q117 Unicode +57.404 ms, Q187 Unicode +27.358 ms, Q116 Unicode +16.519 ms. The [alternating targeted retime](targeted/SUMMARY.md) ran baseline–corrected–baseline–corrected, ten repetitions per process, on five suspect cases under both cache-off alphabets. Q117's two Unicode paired deltas were +3.798 and +10.075 ms with overlapping sample ranges; Q187 was faster in both pairs; Q116 changed direction. The large first-run slowdowns therefore did not reproduce. Q074 ASCII showed a small variable +1.966/+0.710 ms with overlapping ranges. No extra source work or specific avoidable overhead was established. The warm-profile +5.989 to +7.225 ms sums are small relative to individual high-cost cases and are descriptive single-process observations.

## Construction and restore

Times below are one fresh build and explicit save per alphabet/revision; restore is the median of five new processes. `build` excludes the explicit save. Source file and record counts, 148,486 successful parses, zero failed parses, 15 bigram stores and distinct gram-key counts match. Snapshot bytes are identical by alphabet.

| Alphabet | Build baseline→corrected ms | Extraction/index baseline→corrected ms | Finalization baseline→corrected ms | Save baseline→corrected ms | Restore baseline→corrected ms | Snapshot bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII | 2563.376→1190.780 | 703.369→627.478 | 253.039→254.891 | 340.525→230.783 | 573.170→541.131 | 117262305 |
| Unicode | 1236.467→1261.194 | 649.174→690.873 | 259.992→264.311 | 348.185→227.521 | 581.711→547.631 | 119772881 |

The first baseline ASCII process spent 1492.241 ms in discovery, versus 218.533 ms in its later Unicode phase and 223.049/227.762 ms in corrected ASCII/Unicode phases. That asymmetry is consistent with cold-cache or other first-process effects; the 1372.596 ms ASCII build delta is **not** an attributable code speedup. Unicode build and extraction/index differences have only one sample each. Save and restore differences may also reflect cache/write conditions. Process peak RSS at capture was 598.438→586.535 MiB for ASCII and 889.871→858.441 MiB for Unicode. The Unicode peak is the high-water mark of a process that previously built ASCII, so it is not an isolated Unicode-construction peak.

This recording establishes unchanged answers and indexes, substantial uncached computed-sort gains, largely flat warm profiles, and no reproducible large broad-scan regression. It does not isolate an index-build speedup.
