# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q008 | ascii-off | 80.915 | 80.915 | 80.915 | 80.915 | 0 | 0 | 0 | 0/1 | ok |
| Q008 | unicode-off | 68.039 | 68.039 | 68.039 | 68.039 | 0 | 0 | 0 | 0/1 | ok |

## Index construction

- "ascii": build 0.956 s; parses 148486; resident 888 MiB; peak resident 928 MiB; estimated index 0 MiB; rkyv archive 67 MiB.
- "unicode": build 0.948 s; parses 148486; resident 1152 MiB; peak resident 1170 MiB; estimated index 0 MiB; rkyv archive 67 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
