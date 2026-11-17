# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6773.907 | 6773.907 | 6773.907 | 6773.907 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 7301.477 | 7301.477 | 7301.477 | 7301.477 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 764.760 ms; combined 764.872 ms; explicit archive disabled ms; parses 148486; native-ready RSS 696 MiB; archived RSS 696 MiB; peak resident 737 MiB; estimated index 99 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
