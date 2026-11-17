# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 8419.731 | 8419.731 | 8419.731 | 8419.731 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 6446.958 | 6446.958 | 6446.958 | 6446.958 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 821.324 ms; combined 934.605 ms; explicit archive 113.210 ms; parses 148486; native-ready RSS 693 MiB; archived RSS 917 MiB; peak resident 917 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
