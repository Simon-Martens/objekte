# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14495.274 | 14921.526 | 17593.891 | 17593.891 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 358781.396 | 360296.024 | 373142.247 | 373142.247 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2904.857 | 3190.040 | 4025.920 | 4025.920 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 78.302 | 81.358 | 143.635 | 143.635 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 36.259 | 37.075 | 123.219 | 123.219 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 64.186 | 66.116 | 94.154 | 94.154 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4350.302 | 4569.393 | 5003.857 | 5003.857 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2859.746 | 2923.270 | 3167.024 | 3167.024 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2257.391 | 2294.462 | 2720.680 | 2720.680 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6388.658 | 6807.011 | 7127.689 | 7127.689 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 34772.092 | 35060.445 | 36060.183 | 36060.183 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.763 s; parses 148486; resident 681 MiB; peak resident 723 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
