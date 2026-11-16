# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 11901.787 | 12020.305 | 19175.340 | 19175.340 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q119 | ascii-off | 4534.252 | 4567.434 | 5087.862 | 5087.862 | 133948 | 133948 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.619 s; parses 148486; resident 560 MiB; peak resident 601 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
