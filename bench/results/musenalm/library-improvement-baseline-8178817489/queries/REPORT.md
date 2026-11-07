# Musenalm query checkpoint

Code revision: `81788174898c27b2c2c367319bad20ecee450562`. Repetitions: 6. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 31.542 | 37.286 | 119.674 | 124 | 124 | 0 | 5/6 | 0/6 | ok |
| Q045 | ascii-off | 138.228 | 175.659 | 442.564 | 1107 | 1107 | 0 | 5/6 | 0/6 | ok |
| Q084 | ascii-off | 180.195 | 208.782 | 309.637 | 4922 | 4922 | 0 | 5/6 | 0/6 | ok |
| Q127 | ascii-off | 52.455 | 60.849 | 84.048 | 203 | 203 | 0 | 5/6 | 0/6 | ok |
| Q189 | ascii-off | 4416.618 | 6325.927 | 11415.106 | 7 | 203 | 203 | 5/6 | 0/6 | ok |
| Q190 | ascii-off | 103131.142 | 114185.788 | 133227.348 | 3821 | 3821 | 3821 | 5/6 | 0/6 | ok |
| Q194 | ascii-off | 300.678 | 361.070 | 446.109 | 7 | 203 | 203 | 6/6 | 0/6 | ok |

## Index construction

- "ascii": build 1.214 s; snapshot write 0.681 s; parses 148486; resident 522 MiB; peak resident 613 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.556/0.562/0.632 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
