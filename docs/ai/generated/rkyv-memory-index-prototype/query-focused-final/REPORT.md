# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12112.205 | 13173.770 | 56355.087 | 56355.087 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q119 | ascii-off | 5558.882 | 5770.630 | 16318.600 | 16318.600 | 133948 | 133948 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.977 s; parses 148486; resident 901 MiB; peak resident 941 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
