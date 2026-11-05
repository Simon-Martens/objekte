# Zero-cache query candidates: current branch versus master

This follow-up repeats the configured-schema ASCII `json-query-mix` workload at **0 MiB query cache**. It compares master production revision `2d6d876ee7f308890ee19868d18ad26ebea31cc3` with current production revision `90b8db3c7e67d5102355e7525da90a8733ce890b`. The later `7bc3952028` commit contains only the preceding benchmark report. The release binaries are the same preserved executables used for that report; their hashes and the exact workload checksum are in [manifest.json](manifest.json). No production source or benchmark executable changed for this run.

The 152-case workload, Musenalm source paths, metadata, schema, CPU host and compiler are identical to the preceding comparison. The [before](corpora-before.json) and [after](corpora-after.json) corpus fingerprints match each other and the previous recorded fingerprint. Timed processes used 16 Rayon threads pinned to CPUs 0–15. The filesystem page cache was warm. The [runner](../../run_paired_library_matrix.py) alternated current/master, master/current, current/master across **three pairs**; each process warmed the query set and then measured seven rounds. Its exact invocation was:

```sh
python3 crates/bench/run_paired_library_matrix.py \
  --reference target/benchmarks/master-final-20261105/bin/master \
  --candidate target/benchmarks/master-final-20261105/bin/current \
  --expected-input crates/bench/results/master-comparison-20261105/corpora-before.json \
  --actual-input target/benchmarks/master-candidate-count-20261105/corpora-before.json \
  --output target/benchmarks/master-candidate-count-20261105/queries \
  --pairs 3 --rounds 7 --only json-schema-ascii-cache0
```

The runner itself applies `RAYON_NUM_THREADS=16` and `taskset -c 0-15`. The [raw output](raw/) has the six commands, stdout and stderr files, every seven-round sample and the runner's validated signatures. The [table](QUERY-TABLE.md) reports the median of each revision's three invocation medians for every query. Its delta is current minus master; negative means current is faster.

**Candidate scans** means `Metrics::grep_scans`: source candidate records examined by the query executor. It is the work count available consistently from both binaries. It is not the cardinality of every index posting or intermediate bitset. An exact index or metadata path can return results with zero source candidate scans. With the query cache disabled, no sample recorded a verified-result or ordered-result cache hit.

All 152 queries had the same candidate-scan count in all 42 measured samples (seven rounds × six processes). Reported totals and first IDs matched across revisions and pairs. Source reads and parses also matched. Reported totals stop at the existing 10,000 cap, so a row showing 10,000 may have more matching records. The benchmark returns one record and does not assert complete ordered pages.

| Summary of 152 per-query medians | Master | Current | Difference |
| --- | ---: | ---: | ---: |
| Sum | 11,099.690 ms | 10,855.999 ms | −243.690 ms (−2.20%) |

This sum is a compact comparison, not one end-to-end workload timing. It closely matches the previous one-pair zero-cache comparison (−2.43%). Across the 152 distinct query cases, candidate scans sum to 6,019,350; that is repeated per-query work, not a unique record count. Fifty-eight cases have zero source candidate scans because they use paths that do not need source verification.

The largest positive median difference was case 118, `year < 1790 SORT BY @status DESC && year`: 18,863 candidates, a capped 10,000 results and +2.255 ms (+1.35%). Its three paired deltas were +2.255, −0.741 and +3.807 ms, so that difference is not stable. Cases 40 and 41 were +0.309 and +0.291 ms, with identical candidate and result counts. The largest absolute improvements were case 116 (148,478 candidates, −23.036 ms), case 108 (148,478 candidates, −18.752 ms) and case 120 (145,587 candidates, −15.275 ms). No candidate-count or result difference explains the timing changes.
