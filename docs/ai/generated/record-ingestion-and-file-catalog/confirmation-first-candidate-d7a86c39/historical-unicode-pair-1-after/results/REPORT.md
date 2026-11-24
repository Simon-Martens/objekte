# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 11.415 | 12.029 | 107.527 | 107.527 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 20110.983 | 20662.920 | 22727.432 | 22727.432 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 143.727 | 146.657 | 183.640 | 183.640 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 192.608 | 195.927 | 266.950 | 266.950 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 27.275 | 28.140 | 74.542 | 74.542 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 22.557 | 23.142 | 34.581 | 34.581 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 21.187 | 21.450 | 37.363 | 37.363 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 156.552 | 223.051 | 224.740 | 224.740 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.718 s; parses 148486; resident 605 MiB; peak resident 645 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
