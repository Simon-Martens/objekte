# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1405.631 | 1521.308 | 4075.436 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 16770.944 | 18525.650 | 23320.296 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 844.666 | 1030.065 | 1302.065 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 259346.293 | 264085.184 | 269372.216 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1399.559 | 1463.800 | 4058.554 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 16623.737 | 17379.547 | 24065.957 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 336.532 | 359.034 | 3534.109 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 270876.551 | 276351.015 | 289552.704 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.559 s; snapshot write 0.443 s; parses 148486; resident 501 MiB; peak resident 549 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.334/1.345/1.421 s over 5 runs.
- "unicode": build 2.557 s; snapshot write 0.449 s; parses 148486; resident 672 MiB; peak resident 702 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.339/1.352/1.412 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
