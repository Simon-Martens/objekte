# Musenalm query checkpoint

Code revision: `92b8e59ae7efe2a27a36aca2e11c7d9d16119797`. Repetitions: 2. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-warm | 6.965 | 21.576 | 36.187 | 36.187 | 3269 | 0 | 0 | 2/2 | ok |
| Q119 | ascii-warm | 15.687 | 26.726 | 37.764 | 37.764 | 133948 | 0 | 0 | 2/2 | ok |
| Q001 | unicode-warm | 6.832 | 20.312 | 33.792 | 33.792 | 3269 | 0 | 0 | 2/2 | ok |
| Q119 | unicode-warm | 13.061 | 24.836 | 36.610 | 36.610 | 133948 | 0 | 0 | 2/2 | ok |

## Index construction

- "ascii": open/build 0.791 s; parses 148486; resident 681 MiB; peak resident 744 MiB; estimated index 99 MiB; rkyv archive 0 MiB.
- "unicode": open/build 0.821 s; parses 148486; resident 1365 MiB; peak resident 1384 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
