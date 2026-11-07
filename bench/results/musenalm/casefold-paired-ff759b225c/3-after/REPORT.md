# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 10750.815 | 11744.106 | 15427.822 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 17388.447 | 20428.616 | 27408.067 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | ascii-off | 99033.999 | 115875.051 | 129595.432 | 18825 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q078 | ascii-off | 51306.984 | 61913.330 | 91879.708 | 7632 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 5035.741 | 7690.107 | 17230.062 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 2932.876 | 3581.967 | 4211.391 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3208.800 | 3684.421 | 5714.009 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 10718.958 | 11582.464 | 15666.531 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 17111.470 | 20220.767 | 27575.596 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | unicode-off | 110019.587 | 130431.241 | 137359.807 | 18825 | 81159 | 81159 | 10/11 | 0/11 | ok |
| Q078 | unicode-off | 55181.215 | 66564.500 | 88724.060 | 7632 | 81159 | 81159 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2012.193 | 9106.316 | 17822.703 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 3002.835 | 3353.676 | 5059.040 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3760.619 | 6141.721 | 9406.429 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.885 s; snapshot write 0.262 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.482/0.489/0.544 s over 5 runs.
- "unicode": build 12.072 s; snapshot write 0.273 s; parses 148486; resident 324 MiB; peak resident 390 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.490/0.501/0.544 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
