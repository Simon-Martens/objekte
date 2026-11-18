# Musenalm query checkpoint

Code revision: `71636c4fe3343b0b1dea67c0ade31e96b68fca4b`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14313.256 | 14593.900 | 21883.088 | 21883.088 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 352301.732 | 353883.267 | 355917.506 | 355917.506 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2828.582 | 2918.976 | 3208.257 | 3208.257 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 52.987 | 54.397 | 116.826 | 116.826 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 53.672 | 56.176 | 71.440 | 71.440 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 68.773 | 74.214 | 97.738 | 97.738 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4318.857 | 4465.849 | 4651.409 | 4651.409 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2591.223 | 2663.219 | 2789.412 | 2789.412 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2041.305 | 2165.034 | 2356.726 | 2356.726 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6228.840 | 6470.872 | 6632.500 | 6632.500 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 35012.363 | 35253.218 | 36075.921 | 36075.921 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.753 s; parses 148486; resident 662 MiB; peak resident 706 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
