# Musenalm query checkpoint

Code revision: `4d2ac6623abcbed3237d7b56e6ce8f51e5c53a21`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14843.463 | 15121.289 | 18248.158 | 18248.158 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 342773.369 | 343550.747 | 362164.631 | 362164.631 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2752.114 | 2891.625 | 3272.346 | 3272.346 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 51.609 | 54.379 | 116.560 | 116.560 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 55.631 | 56.687 | 72.134 | 72.134 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 68.726 | 71.311 | 95.589 | 95.589 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4072.069 | 4229.272 | 4269.858 | 4269.858 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2415.491 | 2446.022 | 2493.098 | 2493.098 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 1920.635 | 1944.470 | 2262.851 | 2262.851 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 5940.499 | 5993.515 | 6025.303 | 6025.303 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 33244.154 | 33729.873 | 34430.650 | 34430.650 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.718 s; parses 148486; resident 659 MiB; peak resident 701 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
