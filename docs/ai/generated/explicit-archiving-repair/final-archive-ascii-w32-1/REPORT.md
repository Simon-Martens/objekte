# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 8374.039 | 8374.039 | 8374.039 | 8374.039 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 7646.060 | 7646.060 | 7646.060 | 7646.060 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 741.156 ms; combined 850.109 ms; explicit archive 108.876 ms; parses 148486; native-ready RSS 673 MiB; archived RSS 906 MiB; peak resident 906 MiB; estimated index 99 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
