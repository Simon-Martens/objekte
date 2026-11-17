# Musenalm query checkpoint

Code revision: `15f45465ed0b7455577fce5431de50dfeea1ee16`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6825.379 | 6825.379 | 6825.379 | 6825.379 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 5329.854 | 5329.854 | 5329.854 | 5329.854 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 7041.677 | 7041.677 | 7041.677 | 7041.677 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 5253.697 | 5253.697 | 5253.697 | 5253.697 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 797.566 ms; combined 797.625 ms; explicit archive disabled ms; parses 148486; native-ready RSS 675 MiB; archived RSS 675 MiB; peak resident 714 MiB; estimated index 99 MiB; rkyv archive 0 MiB.
- "unicode": native build 809.037 ms; combined 809.101 ms; explicit archive disabled ms; parses 148486; native-ready RSS 1323 MiB; archived RSS 1323 MiB; peak resident 1342 MiB; estimated index 103 MiB; rkyv archive 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
