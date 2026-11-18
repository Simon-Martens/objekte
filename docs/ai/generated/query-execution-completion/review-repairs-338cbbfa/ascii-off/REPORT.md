# Musenalm query checkpoint

Code revision: `338cbbfae7c1878ac12cd7af3d0c0b27015dbc84`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14340.152 | 14546.868 | 19671.667 | 19671.667 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 371407.542 | 379773.589 | 405558.132 | 405558.132 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 3096.411 | 3309.323 | 3884.739 | 3884.739 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 63.327 | 68.842 | 168.250 | 168.250 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 34.820 | 37.362 | 54.116 | 54.116 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 76.018 | 88.482 | 162.148 | 162.148 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4533.929 | 4583.869 | 4664.737 | 4664.737 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2857.058 | 2975.167 | 3189.851 | 3189.851 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2185.142 | 2301.448 | 2542.619 | 2542.619 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 9918.243 | 11928.425 | 15443.452 | 15443.452 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 36011.050 | 39373.597 | 42434.062 | 42434.062 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.768 s; parses 148486; resident 650 MiB; peak resident 694 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
