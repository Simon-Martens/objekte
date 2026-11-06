# Musenalm query checkpoint

Code revision: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 186902.091 | 230096.132 | 239397.002 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 84589.607 | 95812.169 | 127066.225 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q028 | ascii-off | 4268.741 | 6411.882 | 10558.064 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4124.575 | 5595.550 | 15761.513 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | ascii-off | 3357.716 | 4056.447 | 11639.178 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4200.791 | 6293.839 | 9582.412 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | ascii-off | 1486.734 | 1619.350 | 2957.180 | 1003 | 1627 | 1627 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 2188.328 | 4016.415 | 8954.152 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 3688.801 | 6901.865 | 12231.946 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 2922.250 | 3227.486 | 4397.727 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 4272.291 | 5827.883 | 6819.115 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 193070.175 | 241423.144 | 251764.447 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 81446.429 | 94766.474 | 135519.632 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q028 | unicode-off | 3972.272 | 4402.664 | 5872.022 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3161.716 | 4227.152 | 6541.671 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | unicode-off | 4179.703 | 8764.087 | 14369.985 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 4563.717 | 5835.504 | 8095.043 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | unicode-off | 1264.094 | 1496.835 | 2260.906 | 1003 | 1364 | 1364 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2155.183 | 10470.092 | 15932.559 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 2801.253 | 3380.034 | 5173.011 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 2766.343 | 3800.837 | 5345.583 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 5661.810 | 7712.125 | 15720.563 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.343 s; snapshot write 0.288 s; parses 148486; resident 251 MiB; peak resident 339 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.592/0.600/0.643 s over 5 runs.
- "unicode": build 12.240 s; snapshot write 0.299 s; parses 148486; resident 335 MiB; peak resident 407 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.657/0.682/0.707 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
