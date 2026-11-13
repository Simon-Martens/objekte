# Final-code paired warm construction campaign

Seven pairs alternated master and incremental-process order on the same
absolute Musenalm corpus root. Each binary ran from its own checkout in a
fresh process with the warm filesystem cache, ASCII alphabet, 32 build
workers, configured schemas, default inference policy, profiling, snapshot
writing and five query checks. The candidate measured revision was
`526ae413db2acaf87755c3f19b27ab6ede16d9b6`; the master baseline was
`7a6b96684e1fbb849eab7cc20185b942159ed189`. Later query-only commits
may change the final integration tip without changing this construction path.

The binary SHA-256 digests were
`baaa44dd304eea56f24fe69ecf6236adef910dfb99f01008de8303d930bf05b3`
(master) and
`6ed649a276288bfd42dfc339e6744309aa1d83f3dc63b19fd4808682d835e46c`
(candidate). All 14 manifests have identical corpus and schema fingerprints.
`runs.json` embeds every sample and manifest; `summary.json` contains medians,
quartiles and IQRs. The 100+ MB snapshot payloads were deleted after their
byte counts were recorded in the samples. The runner is
[`../2026-11-13-paired-warm/run_pairs.py`](../2026-11-13-paired-warm/run_pairs.py).

| Measure | Master median (IQR) | Candidate median (IQR) | Change |
| --- | ---: | ---: | ---: |
| Full build | 973.344 ms (35.867 ms) | 910.233 ms (34.882 ms) | 6.48% faster |
| Fresh-process high-water RSS | 536.842 MB (5.325 MB) | 568.316 MB (9.161 MB) | 5.86% higher |
| RSS after build | 519.459 MB (5.032 MB) | 558.719 MB (9.216 MB) | 7.56% higher |
| Estimated live index | 69.663 MB (0) | 69.966 MB (0) | 0.44% higher |
| Construction parses | 148,486 (0) | 148,486 (0) | Equal |

The candidate made exactly 148,486 construction source reads and consumed
193,297,230 source bytes in every run: one read/parse per physical file.
It published eight immutable segments. Its median observed catalogue time was
208.079 ms, first searchable batch 286.082 ms and first nonempty query result
286.264 ms. The master has no early publication. Median completed snapshot
size was 178,287,528 bytes for the candidate versus 129,139,458 bytes for
master, a 38.1% storage increase outside the stated build/RSS/live-index
budgets.

Reproduce from the integration checkout:

```text
python3 bench/results/incremental-index-availability/2026-11-13-paired-warm/run_pairs.py --baseline-bin /home/simon/source/objekte/target/release/examples/bench_index_construction --candidate-bin target/release/examples/bench_index_construction --root /home/simon/source/objekte/examples/musenalm --output /tmp/objekte-plan1-final-pairs-reproduction --pairs 7
```
