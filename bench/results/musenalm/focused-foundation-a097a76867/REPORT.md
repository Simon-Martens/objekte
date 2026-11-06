# Musenalm query checkpoint

Code revision: `a097a76867e3ae1bada4dcdda70957e69fddee0c`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 16471.390 | 19072.978 | 24788.746 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 18047.275 | 20180.088 | 23640.268 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3859.960 | 4559.561 | 5172.477 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q115 | ascii-off | 4232.871 | 4561.286 | 5758.874 | 244 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 8124.044 | 10395.089 | 14137.614 | 3269 | 3424 | 3424 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 15097.484 | 17983.672 | 22114.277 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 4250.994 | 5347.699 | 6720.633 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q115 | unicode-off | 4337.068 | 4758.401 | 5269.760 | 244 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 17.082 s; snapshot write 0.314 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.822/0.835/0.863 s over 5 runs.
- "unicode": build 19.454 s; snapshot write 0.421 s; parses 148486; resident 332 MiB; peak resident 404 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.932/0.941/0.952 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
