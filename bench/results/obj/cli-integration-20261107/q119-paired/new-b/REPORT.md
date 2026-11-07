# Musenalm query checkpoint

Code revision: `b8785850373ac27fdf2cedb65f6e2d0883c6307e`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 7402.115 | 8018.907 | 10466.840 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "ascii": build 1.175 s; snapshot write 0.294 s; parses 148486; resident 501 MiB; peak resident 592 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.519/0.523/0.537 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
