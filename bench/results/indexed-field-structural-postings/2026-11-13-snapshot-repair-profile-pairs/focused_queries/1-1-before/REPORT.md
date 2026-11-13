# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 4505.545 | 4776.392 | 6965.648 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4693.678 | 5154.077 | 8895.608 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1588.240 | 1813.967 | 2613.464 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2832.087 | 2990.818 | 3445.937 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4245.519 | 4578.311 | 5874.020 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4558.173 | 5232.687 | 8493.353 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1633.293 | 1914.737 | 2675.149 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2647.433 | 2962.780 | 3159.159 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.980 s; snapshot write 0.306 s; parses 148486; resident 496 MiB; peak resident 518 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.614/0.625/0.680 s over 5 runs.
- "unicode": build 0.991 s; snapshot write 0.300 s; parses 148486; resident 671 MiB; peak resident 671 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.628/0.635/0.673 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
