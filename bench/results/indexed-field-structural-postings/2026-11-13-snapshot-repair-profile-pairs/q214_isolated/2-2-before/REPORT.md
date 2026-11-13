# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q214 | ascii-off | 2449.482 | 2856.349 | 5351.030 | 133948 | 133948 | 0 | 14/15 | 0/15 | ok |

## Index construction

- "ascii": build 1.028 s; snapshot write 0.251 s; parses 148486; resident 478 MiB; peak resident 501 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.613/0.630/0.682 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
