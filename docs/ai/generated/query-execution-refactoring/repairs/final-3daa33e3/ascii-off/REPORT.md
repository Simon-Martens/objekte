# Musenalm query checkpoint

Code revision: `3daa33e381840636cd2b33169cc746f6714ef343`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 15351.374 | 15662.851 | 19211.560 | 19211.560 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 340988.690 | 345371.182 | 358133.896 | 358133.896 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2855.827 | 2979.791 | 3291.274 | 3291.274 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 55.886 | 58.424 | 132.139 | 132.139 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 56.881 | 58.879 | 75.047 | 75.047 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 68.179 | 72.741 | 106.917 | 106.917 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4372.234 | 4406.735 | 4680.703 | 4680.703 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2685.784 | 2755.310 | 3005.336 | 3005.336 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2056.145 | 2128.642 | 2302.398 | 2302.398 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6149.324 | 6415.293 | 6641.070 | 6641.070 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 33830.810 | 34304.094 | 35056.488 | 35056.488 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.735 s; parses 148486; resident 636 MiB; peak resident 679 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
