# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q093 | unicode-off | 23.810 | 27.153 | 96.876 | 96.876 | 3856 | 3856 | 0 | 0/7 | ok |
| Q097 | unicode-off | 56.169 | 57.059 | 80.513 | 80.513 | 0 | 0 | 0 | 0/7 | ok |
| Q107 | unicode-off | 80.232 | 87.182 | 128.518 | 128.518 | 2684 | 2684 | 0 | 0/7 | ok |
| Q115 | unicode-off | 21.453 | 22.170 | 68.278 | 68.278 | 244 | 244 | 0 | 0/7 | ok |
| Q121 | unicode-off | 27.967 | 28.789 | 60.917 | 60.917 | 341 | 341 | 0 | 0/7 | ok |
| Q137 | unicode-off | 174.839 | 178.203 | 348.481 | 348.481 | 81 | 81 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.642 s; parses 148486; resident 569 MiB; peak resident 609 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
