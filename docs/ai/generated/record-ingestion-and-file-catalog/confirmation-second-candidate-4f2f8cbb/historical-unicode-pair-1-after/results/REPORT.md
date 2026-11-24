# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q093 | unicode-off | 24.854 | 26.159 | 96.430 | 96.430 | 3856 | 3856 | 0 | 0/7 | ok |
| Q097 | unicode-off | 56.352 | 58.245 | 81.385 | 81.385 | 0 | 0 | 0 | 0/7 | ok |
| Q107 | unicode-off | 81.563 | 85.435 | 127.781 | 127.781 | 2684 | 2684 | 0 | 0/7 | ok |
| Q115 | unicode-off | 23.895 | 24.491 | 49.966 | 49.966 | 244 | 244 | 0 | 0/7 | ok |
| Q121 | unicode-off | 30.011 | 30.675 | 75.059 | 75.059 | 341 | 341 | 0 | 0/7 | ok |
| Q137 | unicode-off | 176.241 | 179.626 | 348.193 | 348.193 | 81 | 81 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.674 s; parses 148486; resident 605 MiB; peak resident 645 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
