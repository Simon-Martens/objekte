# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 6909.829 | 6909.829 | 6909.829 | 6909.829 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5268.884 | 5268.884 | 5268.884 | 5268.884 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 823.043 ms; combined 823.135 ms; explicit archive disabled ms; parses 148486; native-ready RSS 691 MiB; archived RSS 691 MiB; peak resident 732 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
