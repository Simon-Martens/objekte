# Musenalm query checkpoint

Code revision: `92b8e59ae7efe2a27a36aca2e11c7d9d16119797`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 45816.728 | 45816.728 | 45816.728 | 45816.728 | 133948 | 133948 | 0 | 0/1 | ok |
| Q119 | unicode-off | 34800.390 | 34800.390 | 34800.390 | 34800.390 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.932 s; parses 148486; resident 912 MiB; peak resident 912 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 1.005 s; parses 148486; resident 1629 MiB; peak resident 1674 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
