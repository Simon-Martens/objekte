# Musenalm query checkpoint

Code revision: `65cb66dc5bac2623a94f74cc4e12b110839ce813`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12254.042 | 13845.349 | 61817.011 | 61817.011 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 4915.053 | 5037.693 | 8288.448 | 8288.448 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 13451.244 | 16581.091 | 75735.541 | 75735.541 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 6909.869 | 7546.275 | 8955.628 | 8955.628 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": open/build 0.819 s; parses 148486; resident 747 MiB; peak resident 776 MiB; estimated index 120 MiB; rkyv archive 69 MiB.
- "unicode": open/build 0.836 s; parses 148486; resident 1174 MiB; peak resident 1192 MiB; estimated index 124 MiB; rkyv archive 69 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
