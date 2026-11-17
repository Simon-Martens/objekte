# Musenalm query checkpoint

Code revision: `cd9b8275cd574df64c2889f2fe74565effbe656b`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 50823.537 | 50823.537 | 50823.537 | 50823.537 | 133948 | 133948 | 0 | 0/1 | ok |
| Q119 | unicode-off | 35052.614 | 35052.614 | 35052.614 | 35052.614 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 1.147 s; parses 148486; resident 893 MiB; peak resident 893 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 1.008 s; parses 148486; resident 1653 MiB; peak resident 1653 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
