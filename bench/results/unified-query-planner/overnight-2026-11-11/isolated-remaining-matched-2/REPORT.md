# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | ascii-off | 6006.564 | 6252.546 | 17285.404 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | ascii-off | 54005.048 | 85177.342 | 178164.446 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |
| Q072 | unicode-off | 5827.688 | 6189.859 | 15899.732 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | unicode-off | 56679.023 | 104528.807 | 184039.914 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.577 s; snapshot write 0.543 s; parses 148486; resident 514 MiB; peak resident 552 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.335/1.346/1.439 s over 5 runs.
- "unicode": build 2.649 s; snapshot write 0.550 s; parses 148486; resident 714 MiB; peak resident 714 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.338/1.351/1.452 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
