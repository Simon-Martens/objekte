# Musenalm query checkpoint

Code revision: `92b8e59ae7efe2a27a36aca2e11c7d9d16119797`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 7304.588 | 7304.588 | 7304.588 | 7304.588 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 5130.722 | 5130.722 | 5130.722 | 5130.722 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 6899.478 | 6899.478 | 6899.478 | 6899.478 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5208.370 | 5208.370 | 5208.370 | 5208.370 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.777 s; parses 148486; resident 687 MiB; peak resident 750 MiB; estimated index 99 MiB; rkyv archive 0 MiB.
- "unicode": open/build 0.876 s; parses 148486; resident 1362 MiB; peak resident 1381 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
