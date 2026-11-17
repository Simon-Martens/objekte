# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 9474.893 | 9474.893 | 9474.893 | 9474.893 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 6481.243 | 6481.243 | 6481.243 | 6481.243 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 768.333 ms; combined 881.890 ms; explicit archive 113.492 ms; parses 148486; native-ready RSS 687 MiB; archived RSS 911 MiB; peak resident 911 MiB; estimated index 99 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
