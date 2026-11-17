# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 8766.325 | 8766.325 | 8766.325 | 8766.325 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 6651.205 | 6651.205 | 6651.205 | 6651.205 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 782.639 ms; combined 895.202 ms; explicit archive 112.503 ms; parses 148486; native-ready RSS 689 MiB; archived RSS 912 MiB; peak resident 912 MiB; estimated index 99 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
