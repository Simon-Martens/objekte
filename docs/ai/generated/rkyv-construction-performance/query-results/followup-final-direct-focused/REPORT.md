# Musenalm query checkpoint

Code revision: `65cb66dc5bac2623a94f74cc4e12b110839ce813`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12217.923 | 12716.306 | 80035.185 | 80035.185 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 4938.162 | 4961.245 | 5846.724 | 5846.724 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 13454.437 | 13729.942 | 78957.717 | 78957.717 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 5780.889 | 6937.371 | 9005.410 | 9005.410 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.843 s; parses 148486; resident 721 MiB; peak resident 784 MiB; estimated index 0 MiB; rkyv archive 141 MiB.
- "unicode": build 0.847 s; parses 148486; resident 950 MiB; peak resident 968 MiB; estimated index 0 MiB; rkyv archive 141 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
