# Musenalm query checkpoint

Code revision: `cd9b8275cd574df64c2889f2fe74565effbe656b`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 51703.478 | 51703.478 | 51703.478 | 51703.478 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 5832.947 | 5832.947 | 5832.947 | 5832.947 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 71928.412 | 71928.412 | 71928.412 | 71928.412 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5922.943 | 5922.943 | 5922.943 | 5922.943 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.947 s; parses 148486; resident 909 MiB; peak resident 909 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": open/build 0.976 s; parses 148486; resident 1676 MiB; peak resident 1676 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
