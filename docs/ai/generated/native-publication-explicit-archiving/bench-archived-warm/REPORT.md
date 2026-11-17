# Musenalm query checkpoint

Code revision: `92b8e59ae7efe2a27a36aca2e11c7d9d16119797`. Repetitions: 2. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-warm | 7.379 | 23.789 | 40.199 | 40.199 | 3269 | 0 | 0 | 2/2 | ok |
| Q119 | ascii-warm | 13.383 | 22.815 | 32.246 | 32.246 | 133948 | 0 | 0 | 2/2 | ok |
| Q001 | unicode-warm | 10.389 | 20.525 | 30.661 | 30.661 | 3269 | 0 | 0 | 2/2 | ok |
| Q119 | unicode-warm | 14.319 | 22.246 | 30.172 | 30.172 | 133948 | 0 | 0 | 2/2 | ok |

## Index construction

- "ascii": open/build 0.956 s; parses 148486; resident 913 MiB; peak resident 913 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 0.966 s; parses 148486; resident 1702 MiB; peak resident 1702 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
