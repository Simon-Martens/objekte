# Musenalm query checkpoint

Code revision: `bf32a30e25001a026c26f4efbe73bfb058f007b3`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 15959.312 | 17118.229 | 21831.639 | 21831.639 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 349761.163 | 352828.129 | 358687.230 | 358687.230 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2771.811 | 2959.375 | 3302.263 | 3302.263 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 54.784 | 61.050 | 124.494 | 124.494 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 57.253 | 57.422 | 74.361 | 74.361 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 68.247 | 73.174 | 95.039 | 95.039 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4578.267 | 4653.145 | 4754.351 | 4754.351 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2728.685 | 2834.673 | 3019.091 | 3019.091 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2102.224 | 2327.105 | 2440.239 | 2440.239 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6392.034 | 6449.708 | 6599.168 | 6599.168 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 33778.605 | 35123.655 | 35571.826 | 35571.826 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.708 s; parses 148486; resident 648 MiB; peak resident 690 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
