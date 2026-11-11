# Musenalm query checkpoint

Code revision: `90b52907c7f9c9364305e38e1b689c9766cc84d0`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q048 | ascii-off | 213.021 | 219.623 | 731.698 | 2377 | 2377 | 0 | 6/7 | 0/7 | ok |
| Q049 | ascii-off | 321.887 | 350.461 | 624.356 | 4702 | 4702 | 0 | 6/7 | 0/7 | ok |
| Q071 | ascii-off | 4294.539 | 4391.461 | 4980.039 | 1003 | 1627 | 1627 | 6/7 | 0/7 | ok |
| Q076 | ascii-off | 31699.221 | 33414.381 | 47094.666 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q135 | ascii-off | 11939.727 | 18462.822 | 20812.928 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q048 | unicode-off | 213.272 | 216.969 | 671.865 | 2377 | 2377 | 0 | 6/7 | 0/7 | ok |
| Q049 | unicode-off | 310.856 | 312.138 | 604.659 | 4702 | 4702 | 0 | 6/7 | 0/7 | ok |
| Q071 | unicode-off | 4228.323 | 4633.377 | 5883.330 | 1003 | 1627 | 1627 | 6/7 | 0/7 | ok |
| Q076 | unicode-off | 31571.240 | 33126.840 | 45508.659 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q135 | unicode-off | 12048.131 | 17229.839 | 23855.960 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.531 s; snapshot write 0.449 s; parses 148486; resident 473 MiB; peak resident 554 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.323/1.338/1.368 s over 5 runs.
- "unicode": build 2.602 s; snapshot write 0.471 s; parses 148486; resident 722 MiB; peak resident 722 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.337/1.345/1.427 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
