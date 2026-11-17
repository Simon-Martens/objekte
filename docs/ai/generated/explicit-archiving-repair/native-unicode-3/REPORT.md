# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 7156.217 | 7156.217 | 7156.217 | 7156.217 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5276.916 | 5276.916 | 5276.916 | 5276.916 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 803.637 ms; combined 803.697 ms; explicit archive disabled ms; parses 148486; native-ready RSS 678 MiB; archived RSS 678 MiB; peak resident 718 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
