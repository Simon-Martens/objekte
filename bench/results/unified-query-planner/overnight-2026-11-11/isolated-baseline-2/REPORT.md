# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q116 | ascii-off | 9686.829 | 11198.909 | 35178.390 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | ascii-off | 376.067 | 388.028 | 862.200 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | ascii-off | 1354.445 | 3193.780 | 5428.790 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |
| Q116 | unicode-off | 10255.527 | 11379.188 | 37688.625 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | unicode-off | 402.426 | 448.803 | 955.204 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | unicode-off | 2599.343 | 4579.002 | 6006.825 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.615 s; snapshot write 0.551 s; parses 148486; resident 482 MiB; peak resident 548 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.319/1.321/1.432 s over 5 runs.
- "unicode": build 2.633 s; snapshot write 0.542 s; parses 148486; resident 716 MiB; peak resident 716 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.333/1.350/1.435 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
