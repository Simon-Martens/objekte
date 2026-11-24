# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 10.834 | 12.844 | 106.612 | 106.612 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 19458.701 | 20594.482 | 21944.655 | 21944.655 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 118.132 | 118.694 | 156.215 | 156.215 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 184.656 | 187.268 | 255.315 | 255.315 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 26.427 | 27.503 | 62.258 | 62.258 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 21.598 | 22.123 | 32.013 | 32.013 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 19.665 | 20.180 | 35.119 | 35.119 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 174.886 | 218.305 | 254.517 | 254.517 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.656 s; parses 148486; resident 563 MiB; peak resident 603 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
