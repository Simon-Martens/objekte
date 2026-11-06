# Musenalm query checkpoint

Code revision: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 192490.860 | 228918.742 | 235866.911 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | ascii-off | 2967.775 | 3208.034 | 3587.870 | 1493 | 1504 | 1504 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 96734.031 | 117941.819 | 128003.608 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4151.684 | 11515.515 | 17944.308 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | ascii-off | 2915.987 | 4417.024 | 6122.800 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4980.900 | 7610.637 | 14290.717 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | ascii-off | 2616.831 | 3251.382 | 11014.973 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1877.660 | 2928.227 | 4180.754 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 15070.043 | 21061.128 | 35595.359 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3730.935 | 5175.848 | 13888.449 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 195590.976 | 240944.410 | 250386.155 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | unicode-off | 3002.897 | 3221.213 | 4986.264 | 1493 | 1503 | 1503 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 95436.248 | 124405.409 | 140888.849 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 4110.672 | 6477.127 | 14519.666 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | unicode-off | 3033.762 | 3689.704 | 9023.820 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 6282.066 | 9069.388 | 17641.473 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | unicode-off | 2101.274 | 2667.269 | 6325.776 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 1925.389 | 2123.521 | 3213.419 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 11623.118 | 16065.207 | 26780.455 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 4000.909 | 6036.523 | 11431.504 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.330 s; snapshot write 0.383 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.606/0.616/0.669 s over 5 runs.
- "unicode": build 12.437 s; snapshot write 0.331 s; parses 148486; resident 335 MiB; peak resident 407 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.665/0.673/0.767 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
