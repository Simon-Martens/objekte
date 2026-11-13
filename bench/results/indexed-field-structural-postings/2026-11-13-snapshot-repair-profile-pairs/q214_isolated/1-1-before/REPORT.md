# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q214 | ascii-off | 2788.792 | 3111.919 | 5853.975 | 133948 | 133948 | 0 | 14/15 | 0/15 | ok |

## Index construction

- "ascii": build 1.029 s; snapshot write 0.265 s; parses 148486; resident 495 MiB; peak resident 517 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.615/0.643/0.685 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
