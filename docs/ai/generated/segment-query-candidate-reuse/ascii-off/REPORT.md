# Musenalm query checkpoint

Code revision: `98b5d3a23af28071cdd7f5ad01186e46f8c21702`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14230.628 | 15520.490 | 20001.175 | 20001.175 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 355920.853 | 363821.161 | 371472.424 | 371472.424 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 3389.890 | 3449.473 | 4040.998 | 4040.998 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 63.909 | 65.974 | 146.598 | 146.598 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 35.536 | 36.741 | 55.641 | 55.641 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 82.154 | 88.077 | 126.197 | 126.197 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4507.510 | 4747.940 | 4934.028 | 4934.028 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2774.076 | 2865.841 | 2983.475 | 2983.475 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2104.656 | 2225.659 | 2606.414 | 2606.414 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6714.353 | 6765.523 | 7172.096 | 7172.096 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 35309.999 | 35810.987 | 36041.830 | 36041.830 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.725 s; parses 148486; resident 650 MiB; peak resident 691 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
