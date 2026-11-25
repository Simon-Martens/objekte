# Musenalm query checkpoint

Code revision: `732725bcfba34e11f6b85b0125e0e447151fb03a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 35854.626 | 36819.157 | 41236.957 | 41236.957 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8082.787 | 8123.917 | 9536.618 | 9536.618 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.666 | 4.024 | 27.109 | 27.109 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 21.660 | 23.287 | 43.513 | 43.513 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1531.882 | 1533.608 | 1698.071 | 1698.071 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3150.357 | 3171.800 | 3496.421 | 3496.421 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 194.210 | 195.370 | 244.561 | 244.561 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2642.093 | 2808.668 | 4802.334 | 4802.334 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 61670.311 | 62883.124 | 64476.633 | 64476.633 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.659 s; parses 148486; resident 594 MiB; peak resident 635 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
