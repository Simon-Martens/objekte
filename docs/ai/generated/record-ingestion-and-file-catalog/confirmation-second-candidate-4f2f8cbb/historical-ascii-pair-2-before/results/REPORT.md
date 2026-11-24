# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 280.403 | 282.859 | 399.068 | 399.068 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 105.586 | 106.480 | 152.513 | 152.513 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 16.556 | 17.152 | 32.531 | 32.531 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 304.604 | 306.963 | 464.919 | 464.919 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 70.534 | 77.569 | 107.867 | 107.867 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.817 | 7.199 | 13.660 | 13.660 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.632 s; parses 148486; resident 546 MiB; peak resident 586 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
