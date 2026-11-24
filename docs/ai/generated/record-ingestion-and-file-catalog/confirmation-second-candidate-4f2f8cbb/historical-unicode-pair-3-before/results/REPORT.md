# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q093 | unicode-off | 23.395 | 24.973 | 90.438 | 90.438 | 3856 | 3856 | 0 | 0/7 | ok |
| Q097 | unicode-off | 57.202 | 57.738 | 78.887 | 78.887 | 0 | 0 | 0 | 0/7 | ok |
| Q107 | unicode-off | 79.013 | 84.937 | 126.921 | 126.921 | 2684 | 2684 | 0 | 0/7 | ok |
| Q115 | unicode-off | 22.234 | 22.856 | 43.306 | 43.306 | 244 | 244 | 0 | 0/7 | ok |
| Q121 | unicode-off | 28.284 | 29.156 | 59.220 | 59.220 | 341 | 341 | 0 | 0/7 | ok |
| Q137 | unicode-off | 175.921 | 181.133 | 363.587 | 363.587 | 81 | 81 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.667 s; parses 148486; resident 572 MiB; peak resident 612 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
