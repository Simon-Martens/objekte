# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 7228.237 | 7228.237 | 7228.237 | 7228.237 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5449.189 | 5449.189 | 5449.189 | 5449.189 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "unicode": native build 827.105 ms; combined 827.165 ms; explicit archive disabled ms; parses 148486; native-ready RSS 682 MiB; archived RSS 682 MiB; peak resident 723 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
