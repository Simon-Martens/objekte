# Musenalm query checkpoint

Code revision: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 190166.141 | 231781.113 | 235146.000 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | ascii-off | 3107.272 | 3329.013 | 4234.085 | 1493 | 1504 | 1504 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 96560.597 | 116302.629 | 128187.470 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 3341.334 | 5311.396 | 21438.916 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | ascii-off | 3240.604 | 4303.944 | 4955.703 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 5680.502 | 7579.162 | 14898.650 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | ascii-off | 2188.720 | 2953.173 | 8944.502 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1899.384 | 2226.872 | 3652.050 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 15210.798 | 22465.867 | 32496.365 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3564.525 | 4911.527 | 13824.590 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 195601.204 | 237194.311 | 247307.648 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | unicode-off | 2827.289 | 3051.784 | 4080.682 | 1493 | 1503 | 1503 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 93574.424 | 121297.039 | 138806.598 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3652.941 | 5678.054 | 20118.586 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | unicode-off | 2864.129 | 3550.242 | 4614.202 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 5990.251 | 8710.412 | 17465.212 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | unicode-off | 2185.521 | 2504.173 | 10939.893 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 1948.489 | 2889.007 | 11580.750 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 11113.345 | 17805.608 | 34320.560 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3799.159 | 4733.272 | 7400.404 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.250 s; snapshot write 0.275 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.588/0.593/0.642 s over 5 runs.
- "unicode": build 12.354 s; snapshot write 0.301 s; parses 148486; resident 336 MiB; peak resident 408 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.664/0.672/0.714 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
