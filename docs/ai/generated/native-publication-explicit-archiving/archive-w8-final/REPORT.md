# Musenalm query checkpoint

Code revision: `cd9b8275cd574df64c2889f2fe74565effbe656b`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 48616.400 | 48616.400 | 48616.400 | 48616.400 | 133948 | 133948 | 0 | 0/1 | ok |
| Q119 | unicode-off | 39713.147 | 39713.147 | 39713.147 | 39713.147 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.945 s; parses 148486; resident 913 MiB; peak resident 913 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 1.289 s; parses 148486; resident 1693 MiB; peak resident 1693 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
