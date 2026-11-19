# Musenalm query checkpoint

Code revision: `4fb6ffdc9821c336ff205c66064ebf42b45d17a9`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 11898.738 | 12392.193 | 15874.399 | 15874.399 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q042 | ascii-off | 99.718 | 107.127 | 159.974 | 159.974 | 1520 | 1520 | 0 | 0/3 | ok |
| Q083 | ascii-off | 6749.546 | 6984.134 | 7366.471 | 7366.471 | 4040 | 4928 | 4928 | 0/3 | ok |
| Q136 | ascii-off | 2687.009 | 3842.377 | 4588.044 | 4588.044 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.588 s; parses 148486; resident 573 MiB; peak resident 613 MiB; estimated index 105 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
