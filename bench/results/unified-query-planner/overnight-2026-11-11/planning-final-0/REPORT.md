# Musenalm query checkpoint

Code revision: `00f501902c8aee2a66da5cb829b56ab89dbf0ac8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 48.210 | 57.799 | 159.640 | 124 | 124 | 0 | 0/7 | 0/7 | ok |
| Q056 | ascii-off | 1351.870 | 1374.172 | 3833.021 | 11672 | 11672 | 0 | 0/7 | 0/7 | ok |
| Q115 | ascii-off | 104.656 | 106.830 | 180.348 | 244 | 244 | 0 | 0/7 | 0/7 | ok |
| Q120 | ascii-off | 5497.589 | 7911.313 | 9277.009 | 42700 | 42700 | 0 | 0/7 | 0/7 | ok |
| Q141 | ascii-off | 7288.923 | 13966.008 | 21057.851 | 823 | 982 | 982 | 0/7 | 0/7 | ok |
| Q150 | ascii-off | 6197.032 | 6612.032 | 10667.350 | 390 | 457 | 457 | 0/7 | 0/7 | ok |
| Q160 | ascii-off | 313754.417 | 323808.926 | 337090.789 | 70386 | 70390 | 70390 | 0/7 | 0/7 | ok |
| Q175 | ascii-off | 8722.978 | 8958.991 | 9384.410 | 95153 | 95153 | 0 | 0/7 | 0/7 | ok |
| Q189 | ascii-off | 960.805 | 990.792 | 1402.184 | 7 | 203 | 203 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 20619.047 | 34127.996 | 50990.676 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q035 | unicode-off | 47.119 | 50.815 | 163.316 | 124 | 124 | 0 | 0/7 | 0/7 | ok |
| Q056 | unicode-off | 1362.580 | 1365.615 | 3819.646 | 11672 | 11672 | 0 | 0/7 | 0/7 | ok |
| Q115 | unicode-off | 103.615 | 108.013 | 179.888 | 244 | 244 | 0 | 0/7 | 0/7 | ok |
| Q120 | unicode-off | 6224.834 | 7539.034 | 8096.891 | 42700 | 42700 | 0 | 0/7 | 0/7 | ok |
| Q141 | unicode-off | 7469.874 | 11214.559 | 17169.817 | 823 | 982 | 982 | 0/7 | 0/7 | ok |
| Q150 | unicode-off | 5841.495 | 6595.861 | 13818.361 | 390 | 457 | 457 | 0/7 | 0/7 | ok |
| Q160 | unicode-off | 305062.778 | 309720.128 | 315510.106 | 70386 | 70390 | 70390 | 0/7 | 0/7 | ok |
| Q175 | unicode-off | 8837.253 | 9050.673 | 9872.317 | 95153 | 95153 | 0 | 0/7 | 0/7 | ok |
| Q189 | unicode-off | 1070.181 | 1111.167 | 1436.730 | 7 | 203 | 203 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 20780.450 | 31309.150 | 54194.544 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.672 s; snapshot write 0.562 s; parses 148486; resident 528 MiB; peak resident 555 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.327/1.336/1.488 s over 5 runs.
- "unicode": build 2.612 s; snapshot write 0.566 s; parses 148486; resident 718 MiB; peak resident 718 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.342/1.352/1.555 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
