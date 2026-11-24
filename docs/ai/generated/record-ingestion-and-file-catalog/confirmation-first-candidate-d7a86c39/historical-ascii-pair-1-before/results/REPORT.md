# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 13.318 | 15.319 | 73.515 | 73.515 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 11.112 | 11.777 | 52.604 | 52.604 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 70.684 | 79.808 | 111.691 | 111.691 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 188.301 | 196.540 | 270.133 | 270.133 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 45.749 | 47.546 | 102.784 | 102.784 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 123.570 | 124.840 | 150.783 | 150.783 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.616 s; parses 148486; resident 573 MiB; peak resident 613 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
