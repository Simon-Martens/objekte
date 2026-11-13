# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14700.308 | 16683.076 | 26246.528 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3246.777 | 3521.587 | 13992.339 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6939.066 | 7460.266 | 9959.900 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6392.728 | 6777.459 | 7524.501 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2378.354 | 2556.544 | 6699.434 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2091.216 | 2156.899 | 4766.536 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 5460.364 | 5517.087 | 6171.318 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4825.957 | 5081.363 | 6695.606 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 23960.826 | 24828.227 | 26937.921 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4438.927 | 4627.876 | 5804.732 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4856.505 | 5353.678 | 7359.614 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 514.111 | 595.923 | 1524.566 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6407.037 | 6530.785 | 7470.887 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6752.893 | 6800.449 | 6983.355 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3075.529 | 3664.017 | 8293.959 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14979.617 | 15115.488 | 24848.821 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3811.464 | 4748.033 | 9855.628 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6677.365 | 6829.957 | 7990.005 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6373.460 | 6431.750 | 6997.411 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2197.895 | 2351.319 | 6447.977 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2092.244 | 2171.056 | 4491.225 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 5264.094 | 5394.334 | 6050.107 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4422.036 | 4471.414 | 5552.924 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 23273.849 | 23475.892 | 24488.290 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4488.605 | 4622.783 | 5296.918 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 5005.354 | 5641.852 | 7531.522 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 541.454 | 606.506 | 1253.227 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6359.011 | 6491.388 | 7023.605 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6871.629 | 6994.863 | 7184.322 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 3733.246 | 5245.808 | 8429.600 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.089 s; snapshot write 0.276 s; parses 148486; resident 484 MiB; peak resident 507 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.621/0.649/0.681 s over 5 runs.
- "unicode": build 1.061 s; snapshot write 0.312 s; parses 148486; resident 655 MiB; peak resident 655 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.651/0.686/0.731 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
