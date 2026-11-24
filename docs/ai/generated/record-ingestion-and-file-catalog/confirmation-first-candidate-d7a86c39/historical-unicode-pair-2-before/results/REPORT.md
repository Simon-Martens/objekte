# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q061 | unicode-off | 11.042 | 11.651 | 98.942 | 98.942 | 1 | 1 | 1 | 0/7 | ok |
| Q069 | unicode-off | 19787.249 | 20060.812 | 22121.161 | 22121.161 | 4010 | 4010 | 0 | 0/7 | ok |
| Q111 | unicode-off | 117.214 | 118.229 | 154.045 | 154.045 | 4928 | 4928 | 0 | 0/7 | ok |
| Q116 | unicode-off | 187.190 | 188.509 | 267.314 | 267.314 | 70428 | 70428 | 0 | 0/7 | ok |
| Q121 | unicode-off | 26.672 | 27.365 | 63.803 | 63.803 | 341 | 341 | 0 | 0/7 | ok |
| Q122 | unicode-off | 21.826 | 22.595 | 33.239 | 33.239 | 67 | 67 | 0 | 0/7 | ok |
| Q123 | unicode-off | 20.087 | 20.483 | 34.518 | 34.518 | 4153 | 4153 | 0 | 0/7 | ok |
| Q124 | unicode-off | 157.108 | 216.989 | 220.751 | 220.751 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.660 s; parses 148486; resident 571 MiB; peak resident 611 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
