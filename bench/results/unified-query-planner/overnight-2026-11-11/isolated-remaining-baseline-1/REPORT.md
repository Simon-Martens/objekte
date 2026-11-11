# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | ascii-off | 5658.971 | 6056.538 | 16613.090 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | ascii-off | 52371.881 | 81393.424 | 160991.804 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |
| Q072 | unicode-off | 5738.260 | 6243.229 | 16688.112 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | unicode-off | 53487.897 | 106178.927 | 175838.716 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.573 s; snapshot write 0.546 s; parses 148486; resident 538 MiB; peak resident 565 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.323/1.338/1.489 s over 5 runs.
- "unicode": build 2.607 s; snapshot write 0.557 s; parses 148486; resident 714 MiB; peak resident 714 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.354/1.357/1.466 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
