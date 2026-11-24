# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q039 | unicode-off | 23.608 | 25.244 | 76.487 | 76.487 | 0 | 0 | 0 | 0/7 | ok |
| Q050 | unicode-off | 271.272 | 277.775 | 359.975 | 359.975 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | unicode-off | 102.560 | 104.431 | 152.182 | 152.182 | 1942 | 1942 | 0 | 0/7 | ok |
| Q056 | unicode-off | 92.831 | 94.651 | 135.119 | 135.119 | 11672 | 11672 | 0 | 0/7 | ok |
| Q061 | unicode-off | 10.536 | 11.370 | 59.493 | 59.493 | 1 | 1 | 1 | 0/7 | ok |
| Q091 | unicode-off | 24.099 | 24.686 | 49.940 | 49.940 | 3911 | 3911 | 0 | 0/7 | ok |
| Q108 | unicode-off | 37.955 | 39.297 | 63.175 | 63.175 | 2684 | 2684 | 0 | 0/7 | ok |
| Q109 | unicode-off | 78.592 | 83.449 | 122.344 | 122.344 | 2651 | 2651 | 0 | 0/7 | ok |
| Q110 | unicode-off | 22.412 | 22.677 | 46.091 | 46.091 | 4911 | 4911 | 0 | 0/7 | ok |
| Q124 | unicode-off | 169.696 | 227.495 | 232.927 | 232.927 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.656 s; parses 148486; resident 572 MiB; peak resident 612 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
