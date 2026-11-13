# Musenalm query checkpoint

Code revision: `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 4616.636 | 5342.885 | 8508.512 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4455.296 | 4981.117 | 12111.282 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1762.664 | 2019.984 | 3409.457 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3341.083 | 3732.953 | 5062.794 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4232.754 | 4779.189 | 8215.042 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4700.053 | 5275.920 | 8359.690 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1752.602 | 1937.902 | 2667.174 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2600.418 | 3191.059 | 3417.738 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.015 s; snapshot write 0.249 s; parses 148486; resident 494 MiB; peak resident 516 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.618/0.650/0.703 s over 5 runs.
- "unicode": build 0.945 s; snapshot write 0.259 s; parses 148486; resident 655 MiB; peak resident 655 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.624/0.635/0.702 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
