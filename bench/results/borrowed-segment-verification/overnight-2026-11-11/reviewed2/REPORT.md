# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1929.554 | 2459.150 | 3937.025 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 16779.700 | 17657.599 | 23971.290 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 893.518 | 1046.566 | 1459.301 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 285180.872 | 290503.910 | 298699.846 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1362.750 | 2783.508 | 4041.622 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 15805.800 | 16423.270 | 21585.778 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 361.659 | 510.879 | 1593.604 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 249421.078 | 256613.860 | 274401.152 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.519 s; snapshot write 0.440 s; parses 148486; resident 450 MiB; peak resident 535 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.332/1.339/1.365 s over 5 runs.
- "unicode": build 2.576 s; snapshot write 0.459 s; parses 148486; resident 663 MiB; peak resident 663 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.336/1.353/1.426 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
