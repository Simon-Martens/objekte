# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 11.302 | 12.260 | 107.853 | 107.853 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 19902.758 | 20387.953 | 22612.419 | 22612.419 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 143.751 | 146.185 | 184.719 | 184.719 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 191.898 | 194.414 | 257.983 | 257.983 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 27.394 | 28.546 | 71.051 | 71.051 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 22.382 | 22.703 | 35.280 | 35.280 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 21.192 | 21.888 | 34.917 | 34.917 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 219.881 | 225.532 | 242.528 | 242.528 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.720 s; parses 148486; resident 593 MiB; peak resident 633 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
