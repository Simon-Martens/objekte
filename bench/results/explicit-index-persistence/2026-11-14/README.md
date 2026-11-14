# Explicit index persistence measurement archive

Task: `explicit-index-persistence`. Measured 2026-11-14 on Linux 7.2.15, Intel i9-13900K, Rust 1.99.1. The baseline is the archived ordinary release executable from the direct-snapshot-access candidate; executable hashes are in `paired-storage.json` and each run manifest. The candidate is `integration/explicit-index-persistence`. All trials use the same Musenalm source tree (148,486 records), 32 query workers, ASCII where a single profile is specified, and the same machine. Stores and disposable mutation fixtures are under the integration worktree's ignored `target/explicit-index-persistence/` directory.

## Results

| Paired operation, three trials per revision | Baseline median | Candidate median | Candidate / baseline |
| --- | ---: | ---: | ---: |
| Incremental build plus saved generation, ASCII | 1,806.797 ms | 1,498.619 ms | 0.829 |
| Source-free on-demand open, ASCII | 249.022 ms | 252.732 ms | 1.015 |
| Gross bytes allocated during open | 248.789 MB | 248.817 MB | 1.000 |

The build-plus-save comparison includes all background writes in the old combined policy and the one explicit save in the new contract. The isolated `flush_ns` fields in `paired-storage.json` are **not** a comparable phase: the baseline automatically committed several generations during construction (its final generation was 10), whereas the candidate built in memory and committed generation 1 once. The separately measured candidate unchanged-save fast path has a 1.279 ms median after checking the destination manifest; before that fix it took 45.449 ms because it reopened the whole generation. The old implementation skipped this verification and took about 1 µs.

The full query campaign used `bench_musenalm --extended --incremental --storage memory --profiles ascii-off,unicode-off --repetitions 3 --query-workers 32`. Result digests match for all 220 cases in each profile, with zero failed cases. Caching was disabled. Candidate/baseline geometric mean ratios of per-case median latency were 0.944 for ASCII and 1.021 for Unicode. Ratios of summed per-case medians were 1.001 and 0.986 respectively. Short query noise and the three-repetition sample limit make these directional measurements, not a millisecond-level gate.

On three source-free opens of a candidate saved index, the first count query over `agents` had a 0.0177 ms median, returned 4,010 records, and did not read or parse sources. On a disposable 16,384-file JSONL fixture, the first verified update after explicit load was 11.175 ms versus 9.538 ms in one baseline trial. The next candidate update was 5.371 ms versus 7.057 ms baseline. Both selected `VerifiedSlotPreserving`; the first-update difference is one single-run sample, and there was no whole-index lookup rebuild or reopen.

## Reproduction and files

- `baseline/` and `candidate/`: full ASCII/Unicode raw trials, query plans, CSV summaries, result digests, and run manifests.
- `paired-build-save.json`: six alternating total build-and-save trials; each row has the exact command and source-read counters.
- `paired-storage.json`: six alternating old/new flush and open-allocation trials, with binary hashes and exact commands.
- `unchanged-fastpath.json`: three candidate flush trials after the manifest-only unchanged check.
- `first-query.json`: three candidate opens followed by one first query each.
- `first-update-baseline.tsv` and `first-update.tsv`: baseline and candidate disposable-fixture locality runs.
- `total-*/`: individual single-case build-and-save manifests and raw outputs used for the total timing table.

The benchmark commands and root paths are preserved in those JSON manifests. The active examples `bench_musenalm`, `bench_managed_flush`, `bench_managed_open_alloc`, and `bench_lifecycle_mutations` now use explicit load or flush operations and have no fully resident case.
