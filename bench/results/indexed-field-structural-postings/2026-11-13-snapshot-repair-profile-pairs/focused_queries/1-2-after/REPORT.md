# Musenalm query checkpoint

Code revision: `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 4370.879 | 5220.651 | 6109.500 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4545.925 | 5147.102 | 11248.454 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1710.623 | 2046.005 | 3458.152 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3301.375 | 3385.335 | 5087.486 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4590.286 | 4821.328 | 8260.777 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4690.144 | 4881.973 | 12661.543 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1648.293 | 1852.276 | 3585.037 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 3082.225 | 3381.805 | 4938.957 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.048 s; snapshot write 0.296 s; parses 148486; resident 496 MiB; peak resident 518 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.617/0.627/0.684 s over 5 runs.
- "unicode": build 0.977 s; snapshot write 0.299 s; parses 148486; resident 689 MiB; peak resident 689 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.620/0.650/0.695 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
