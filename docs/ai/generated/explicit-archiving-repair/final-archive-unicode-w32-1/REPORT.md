# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 9143.921 | 9143.921 | 9143.921 | 9143.921 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 6375.405 | 6375.405 | 6375.405 | 6375.405 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 842.765 ms; combined 950.739 ms; explicit archive 107.895 ms; parses 148486; native-ready RSS 671 MiB; archived RSS 898 MiB; peak resident 898 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
