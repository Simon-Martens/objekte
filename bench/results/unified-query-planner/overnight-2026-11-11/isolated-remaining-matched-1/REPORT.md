# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | ascii-off | 5674.841 | 6128.274 | 15783.492 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | ascii-off | 52323.319 | 93772.199 | 160228.419 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |
| Q072 | unicode-off | 6030.930 | 6392.800 | 16591.901 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q136 | unicode-off | 53074.390 | 93790.975 | 177018.102 | 133948 | 133948 | 0 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.604 s; snapshot write 0.555 s; parses 148486; resident 532 MiB; peak resident 557 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.329/1.330/1.470 s over 5 runs.
- "unicode": build 2.575 s; snapshot write 0.547 s; parses 148486; resident 767 MiB; peak resident 767 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.344/1.346/1.438 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
