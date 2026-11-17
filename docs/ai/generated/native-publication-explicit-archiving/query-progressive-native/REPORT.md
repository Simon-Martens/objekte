# Musenalm query checkpoint

Code revision: `cd9b8275cd574df64c2889f2fe74565effbe656b`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6980.540 | 6980.540 | 6980.540 | 6980.540 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 6526.014 | 6526.014 | 6526.014 | 6526.014 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 6867.326 | 6867.326 | 6867.326 | 6867.326 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5276.209 | 5276.209 | 5276.209 | 5276.209 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": open/build 0.803 s; parses 148486; resident 682 MiB; peak resident 745 MiB; estimated index 99 MiB; rkyv archive 0 MiB.
- "unicode": open/build 0.808 s; parses 148486; resident 1368 MiB; peak resident 1387 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
