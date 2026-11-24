# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q093 | unicode-off | 24.616 | 25.779 | 92.838 | 92.838 | 3856 | 3856 | 0 | 0/7 | ok |
| Q097 | unicode-off | 56.137 | 56.648 | 81.554 | 81.554 | 0 | 0 | 0 | 0/7 | ok |
| Q107 | unicode-off | 83.026 | 88.072 | 123.795 | 123.795 | 2684 | 2684 | 0 | 0/7 | ok |
| Q115 | unicode-off | 21.194 | 22.834 | 58.444 | 58.444 | 244 | 244 | 0 | 0/7 | ok |
| Q121 | unicode-off | 29.741 | 30.037 | 60.502 | 60.502 | 341 | 341 | 0 | 0/7 | ok |
| Q137 | unicode-off | 173.888 | 181.354 | 355.425 | 355.425 | 81 | 81 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.643 s; parses 148486; resident 566 MiB; peak resident 606 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
