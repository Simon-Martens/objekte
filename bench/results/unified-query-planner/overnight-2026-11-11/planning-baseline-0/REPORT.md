# Musenalm query checkpoint

Code revision: `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 54.613 | 55.515 | 166.463 | 124 | 124 | 0 | 0/7 | 0/7 | ok |
| Q056 | ascii-off | 1346.129 | 1367.198 | 3911.088 | 11672 | 11672 | 0 | 0/7 | 0/7 | ok |
| Q115 | ascii-off | 104.907 | 107.893 | 180.990 | 244 | 244 | 0 | 0/7 | 0/7 | ok |
| Q120 | ascii-off | 3247.320 | 4162.329 | 9166.401 | 42700 | 42700 | 0 | 0/7 | 0/7 | ok |
| Q141 | ascii-off | 7227.819 | 8173.726 | 21976.096 | 823 | 982 | 982 | 0/7 | 0/7 | ok |
| Q150 | ascii-off | 7975.974 | 9966.494 | 16280.766 | 390 | 457 | 457 | 0/7 | 0/7 | ok |
| Q160 | ascii-off | 327508.928 | 340449.779 | 360800.062 | 70386 | 70390 | 70390 | 0/7 | 0/7 | ok |
| Q175 | ascii-off | 8883.860 | 9134.090 | 9638.437 | 95153 | 95153 | 0 | 0/7 | 0/7 | ok |
| Q189 | ascii-off | 961.717 | 1067.426 | 2123.540 | 7 | 203 | 203 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 23138.200 | 26153.093 | 56282.447 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q035 | unicode-off | 53.811 | 56.386 | 174.067 | 124 | 124 | 0 | 0/7 | 0/7 | ok |
| Q056 | unicode-off | 1346.851 | 1368.100 | 3878.697 | 11672 | 11672 | 0 | 0/7 | 0/7 | ok |
| Q115 | unicode-off | 111.590 | 113.954 | 190.397 | 244 | 244 | 0 | 0/7 | 0/7 | ok |
| Q120 | unicode-off | 5540.639 | 7460.075 | 9428.042 | 42700 | 42700 | 0 | 0/7 | 0/7 | ok |
| Q141 | unicode-off | 7241.896 | 22887.218 | 29123.554 | 823 | 982 | 982 | 0/7 | 0/7 | ok |
| Q150 | unicode-off | 5063.232 | 6300.748 | 8464.532 | 390 | 457 | 457 | 0/7 | 0/7 | ok |
| Q160 | unicode-off | 332329.584 | 336404.320 | 341439.459 | 70386 | 70390 | 70390 | 0/7 | 0/7 | ok |
| Q175 | unicode-off | 9037.028 | 9340.187 | 10093.993 | 95153 | 95153 | 0 | 0/7 | 0/7 | ok |
| Q189 | unicode-off | 1054.011 | 1082.123 | 1493.025 | 7 | 203 | 203 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 22109.567 | 35544.237 | 49649.977 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.606 s; snapshot write 0.545 s; parses 148486; resident 542 MiB; peak resident 568 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.329/1.332/1.452 s over 5 runs.
- "unicode": build 2.634 s; snapshot write 0.557 s; parses 148486; resident 687 MiB; peak resident 687 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.339/1.357/1.492 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
