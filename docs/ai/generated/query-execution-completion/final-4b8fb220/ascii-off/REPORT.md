# Musenalm query checkpoint

Code revision: `4b8fb2207bef169eaa4d8ebf66262fbbc04fa35c`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 15410.997 | 15595.353 | 18358.406 | 18358.406 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 372145.560 | 387065.104 | 407227.309 | 407227.309 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 3079.053 | 3180.856 | 4041.589 | 4041.589 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 68.818 | 69.795 | 126.022 | 126.022 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 31.963 | 34.111 | 54.534 | 54.534 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 83.941 | 87.905 | 106.034 | 106.034 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4465.980 | 4623.295 | 4776.015 | 4776.015 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2889.051 | 2957.247 | 3404.312 | 3404.312 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2031.604 | 2119.803 | 2384.043 | 2384.043 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 8263.224 | 10745.396 | 18933.309 | 18933.309 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 35779.960 | 36896.937 | 49045.200 | 49045.200 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.792 s; parses 148486; resident 669 MiB; peak resident 712 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
