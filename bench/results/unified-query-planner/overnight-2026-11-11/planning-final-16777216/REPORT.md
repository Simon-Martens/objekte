# Musenalm query checkpoint

Code revision: `00f501902c8aee2a66da5cb829b56ab89dbf0ac8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 46.567 | 48.771 | 154.640 | 124 | 124 | 0 | 6/7 | 0/7 | ok |
| Q056 | ascii-off | 1348.994 | 1360.757 | 3857.897 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q115 | ascii-off | 102.483 | 114.856 | 177.824 | 244 | 244 | 0 | 6/7 | 0/7 | ok |
| Q120 | ascii-off | 5560.687 | 7733.790 | 9036.316 | 42700 | 42700 | 0 | 6/7 | 0/7 | ok |
| Q141 | ascii-off | 7112.332 | 13969.093 | 23295.695 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q150 | ascii-off | 5857.305 | 8161.933 | 9529.463 | 390 | 457 | 457 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 323564.096 | 327488.789 | 331263.031 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 8765.097 | 9004.728 | 9247.513 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 1053.449 | 1075.290 | 1386.254 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q190 | ascii-off | 19992.560 | 27122.094 | 63455.002 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |
| Q035 | unicode-off | 46.758 | 50.184 | 193.914 | 124 | 124 | 0 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1354.745 | 1373.901 | 3871.513 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q115 | unicode-off | 103.134 | 105.328 | 186.911 | 244 | 244 | 0 | 6/7 | 0/7 | ok |
| Q120 | unicode-off | 5335.082 | 5911.045 | 8178.675 | 42700 | 42700 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 7402.356 | 9443.822 | 20883.874 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q150 | unicode-off | 5642.020 | 9940.064 | 16161.663 | 390 | 457 | 457 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 320981.766 | 329350.688 | 338408.765 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 8538.962 | 8887.997 | 9638.819 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 1043.421 | 1137.608 | 1623.771 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q190 | unicode-off | 22175.832 | 31740.290 | 61020.669 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.651 s; snapshot write 0.535 s; parses 148486; resident 484 MiB; peak resident 541 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.334/1.339/1.437 s over 5 runs.
- "unicode": build 2.627 s; snapshot write 0.546 s; parses 148486; resident 787 MiB; peak resident 787 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.347/1.353/1.474 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
