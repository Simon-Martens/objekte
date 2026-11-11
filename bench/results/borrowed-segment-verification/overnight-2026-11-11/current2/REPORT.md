# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1382.747 | 1550.443 | 4022.426 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 16798.085 | 17211.311 | 24640.156 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 347.223 | 382.218 | 1413.295 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 277598.388 | 279077.366 | 289535.231 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1395.071 | 1419.967 | 3993.692 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 16363.909 | 16977.472 | 22627.515 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 337.755 | 432.973 | 2096.919 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 250224.717 | 256281.224 | 270941.434 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.558 s; snapshot write 0.444 s; parses 148486; resident 526 MiB; peak resident 552 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.328/1.346/1.395 s over 5 runs.
- "unicode": build 2.609 s; snapshot write 0.452 s; parses 148486; resident 702 MiB; peak resident 702 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.326/1.339/1.418 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
