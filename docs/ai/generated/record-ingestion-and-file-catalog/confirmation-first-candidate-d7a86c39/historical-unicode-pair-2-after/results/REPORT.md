# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 10.989 | 11.991 | 109.957 | 109.957 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 19988.188 | 20568.515 | 22570.405 | 22570.405 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 143.686 | 145.485 | 181.874 | 181.874 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 191.198 | 197.152 | 266.876 | 266.876 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 27.044 | 27.578 | 76.538 | 76.538 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 22.576 | 22.977 | 34.710 | 34.710 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 21.216 | 21.486 | 39.433 | 39.433 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 150.487 | 221.815 | 223.314 | 223.314 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.706 s; parses 148486; resident 610 MiB; peak resident 651 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
