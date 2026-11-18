# Musenalm query checkpoint

Code revision: `67ec75be400be0127d55a40860767775636e3cf4`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14638.059 | 14839.636 | 18497.668 | 18497.668 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 369792.029 | 371471.458 | 395855.475 | 395855.475 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2979.692 | 3073.813 | 4414.277 | 4414.277 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 64.784 | 72.734 | 143.337 | 143.337 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 36.243 | 37.210 | 53.068 | 53.068 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 78.391 | 106.543 | 134.509 | 134.509 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4520.712 | 4647.294 | 4905.299 | 4905.299 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2600.942 | 2672.019 | 2722.147 | 2722.147 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2054.441 | 2139.761 | 2358.041 | 2358.041 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 8942.802 | 9359.833 | 13088.187 | 13088.187 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 36138.671 | 40076.911 | 47971.108 | 47971.108 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.765 s; parses 148486; resident 649 MiB; peak resident 693 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
