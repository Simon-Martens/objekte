# Musenalm query checkpoint

Code revision: `30185637b66a7d2e2f0d5956b5d809b9f3a31da0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 9776.456 | 9776.456 | 9776.456 | 9776.456 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 6430.899 | 6430.899 | 6430.899 | 6430.899 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 767.243 ms; combined 920.519 ms; explicit archive 153.212 ms; parses 148486; native-ready RSS 692 MiB; archived RSS 910 MiB; peak resident 910 MiB; estimated index 99 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
