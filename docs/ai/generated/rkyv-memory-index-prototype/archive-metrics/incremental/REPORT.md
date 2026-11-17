# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q008 | ascii-off | 79.699 | 79.699 | 79.699 | 79.699 | 0 | 0 | 0 | 0/1 | ok |
| Q008 | unicode-off | 54.357 | 54.357 | 54.357 | 54.357 | 0 | 0 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.940 s; parses 148486; resident 940 MiB; peak resident 946 MiB; estimated index 120 MiB; rkyv archive 141 MiB.
- "unicode": open/build 1.033 s; parses 148486; resident 1250 MiB; peak resident 1268 MiB; estimated index 124 MiB; rkyv archive 141 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
