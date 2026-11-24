# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 11.071 | 12.394 | 107.847 | 107.847 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 19769.915 | 20090.669 | 22145.545 | 22145.545 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 118.007 | 121.704 | 154.528 | 154.528 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 185.797 | 192.342 | 285.605 | 285.605 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 26.385 | 28.640 | 75.940 | 75.940 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 21.719 | 22.229 | 33.515 | 33.515 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 20.031 | 20.965 | 33.869 | 33.869 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 166.990 | 215.953 | 226.337 | 226.337 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.660 s; parses 148486; resident 578 MiB; peak resident 617 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
