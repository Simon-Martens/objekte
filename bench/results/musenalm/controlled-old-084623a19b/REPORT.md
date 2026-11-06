# Musenalm query checkpoint

Code revision: `084623a19bf26fc56b470b6548c38382dff4d880`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 204571.840 | 227345.578 | 245104.347 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q007 | ascii-off | 247577.671 | 286123.941 | 290164.151 | 147798 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 16423.482 | 17906.902 | 26854.553 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 6791.412 | 8299.639 | 18924.164 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 14117.115 | 16798.959 | 25338.970 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 4397.897 | 4950.235 | 7516.950 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 190663.363 | 228255.836 | 237947.913 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q007 | unicode-off | 251697.889 | 269593.434 | 282653.436 | 147798 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 10207.924 | 10462.773 | 12533.605 | 1742 | 1744 | 1744 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 4844.773 | 8482.460 | 22165.647 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 11303.489 | 13854.236 | 25027.821 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 5340.486 | 6795.140 | 11518.261 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 10.863 s; snapshot write 0.257 s; parses 148486; resident 244 MiB; peak resident 327 MiB; estimated index 65 MiB; snapshot 103 MiB; fresh-process restore min/median/max 0.490/0.493/0.537 s over 5 runs.
- "unicode": build 11.830 s; snapshot write 0.679 s; parses 148486; resident 326 MiB; peak resident 398 MiB; estimated index 77 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.494/0.510/0.579 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
