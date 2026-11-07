# Musenalm query checkpoint

Code revision: `b8785850373ac27fdf2cedb65f6e2d0883c6307e`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 7410.887 | 8105.342 | 9937.810 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.181 s; snapshot write 0.254 s; parses 148486; resident 538 MiB; peak resident 628 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.515/0.531/0.551 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
