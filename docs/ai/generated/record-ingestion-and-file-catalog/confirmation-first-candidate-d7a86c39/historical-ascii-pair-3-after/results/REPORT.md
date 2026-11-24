# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 15.181 | 15.410 | 72.344 | 72.344 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 11.599 | 12.564 | 50.752 | 50.752 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 75.019 | 77.556 | 113.650 | 113.650 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 195.456 | 199.740 | 265.610 | 265.610 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 47.017 | 49.494 | 97.378 | 97.378 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 152.378 | 154.024 | 179.394 | 179.394 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.647 s; parses 148486; resident 593 MiB; peak resident 633 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
