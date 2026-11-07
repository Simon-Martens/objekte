# Musenalm query checkpoint

Code revision: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 190665.528 | 229341.713 | 234257.668 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 84779.838 | 92028.022 | 127778.546 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q028 | ascii-off | 4174.505 | 6197.208 | 7266.439 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4781.355 | 5613.073 | 15729.231 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | ascii-off | 3430.390 | 4108.460 | 20844.903 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4566.598 | 5902.109 | 8939.593 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | ascii-off | 1484.385 | 1749.429 | 2005.471 | 1003 | 1627 | 1627 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1960.416 | 3793.709 | 7450.030 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 3973.102 | 5180.198 | 11368.724 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3029.552 | 3571.727 | 5153.373 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 4012.804 | 4668.034 | 7892.191 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 192622.439 | 239560.466 | 249798.585 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 82914.960 | 94567.842 | 132956.954 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q028 | unicode-off | 4384.205 | 4874.493 | 7568.503 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3348.622 | 5706.597 | 12365.163 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | unicode-off | 4901.867 | 10654.046 | 15711.283 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 4550.322 | 4887.485 | 10396.184 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | unicode-off | 1525.908 | 1948.762 | 5487.250 | 1003 | 1364 | 1364 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 1873.107 | 3304.577 | 18763.120 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 2830.027 | 3517.850 | 5462.616 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3106.070 | 4273.363 | 5812.392 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 4704.512 | 5759.253 | 13886.912 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 10.915 s; snapshot write 0.283 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.575/0.586/0.613 s over 5 runs.
- "unicode": build 12.292 s; snapshot write 0.330 s; parses 148486; resident 336 MiB; peak resident 407 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.667/0.673/0.719 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
