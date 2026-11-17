# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 8312.927 | 8312.927 | 8312.927 | 8312.927 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 6533.597 | 6533.597 | 6533.597 | 6533.597 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 822.645 ms; combined 932.745 ms; explicit archive 110.011 ms; parses 148486; native-ready RSS 675 MiB; archived RSS 909 MiB; peak resident 909 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
