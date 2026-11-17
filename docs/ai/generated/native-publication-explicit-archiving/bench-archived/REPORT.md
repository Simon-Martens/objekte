# Musenalm query checkpoint

Code revision: `92b8e59ae7efe2a27a36aca2e11c7d9d16119797`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 52878.347 | 52878.347 | 52878.347 | 52878.347 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 6047.103 | 6047.103 | 6047.103 | 6047.103 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 79681.269 | 79681.269 | 79681.269 | 79681.269 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 6063.332 | 6063.332 | 6063.332 | 6063.332 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.937 s; parses 148486; resident 911 MiB; peak resident 911 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 0.994 s; parses 148486; resident 1658 MiB; peak resident 1658 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
