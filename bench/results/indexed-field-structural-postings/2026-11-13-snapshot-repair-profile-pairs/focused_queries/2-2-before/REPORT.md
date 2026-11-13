# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 4284.148 | 4413.935 | 7305.747 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4806.377 | 4854.244 | 8816.147 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1756.306 | 1935.729 | 2661.708 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2563.228 | 2997.245 | 3211.028 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4101.695 | 4271.159 | 6147.226 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4678.331 | 4937.061 | 8405.324 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1687.125 | 1834.806 | 2785.779 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2848.759 | 3153.956 | 3872.017 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.012 s; snapshot write 0.274 s; parses 148486; resident 481 MiB; peak resident 504 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.615/0.617/0.671 s over 5 runs.
- "unicode": build 0.976 s; snapshot write 0.248 s; parses 148486; resident 658 MiB; peak resident 658 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.622/0.628/0.676 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
