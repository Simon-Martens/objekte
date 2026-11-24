# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 145.788 | 152.463 | 249.786 | 249.786 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5697.992 | 6082.012 | 7313.198 | 7313.198 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 31731.837 | 32022.378 | 34083.050 | 34083.050 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 44.969 | 49.056 | 143.069 | 143.069 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.063 | 28.388 | 45.213 | 45.213 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 156.220 | 158.098 | 196.496 | 196.496 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.776 s; parses 148486; resident 636 MiB; peak resident 677 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
