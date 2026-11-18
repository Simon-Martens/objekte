# Musenalm query checkpoint

Code revision: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 15243.284 | 15456.110 | 18741.444 | 18741.444 | 3269 | 4872 | 4872 | 0/5 | ok |
| Q007 | ascii-off | 341335.775 | 343958.980 | 354826.354 | 354826.354 | 147798 | 148486 | 148486 | 0/5 | ok |
| Q030 | ascii-off | 2870.016 | 2986.369 | 3208.111 | 3208.111 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q035 | ascii-off | 55.003 | 57.716 | 115.961 | 115.961 | 124 | 124 | 0 | 0/5 | ok |
| Q084 | ascii-off | 56.089 | 56.385 | 73.913 | 73.913 | 4922 | 4922 | 0 | 0/5 | ok |
| Q127 | ascii-off | 72.748 | 74.731 | 94.553 | 94.553 | 203 | 203 | 0 | 0/5 | ok |
| Q141 | ascii-off | 4455.801 | 4500.220 | 4528.624 | 4528.624 | 823 | 982 | 982 | 0/5 | ok |
| Q150 | ascii-off | 2726.532 | 2774.718 | 2836.580 | 2836.580 | 390 | 457 | 457 | 0/5 | ok |
| Q163 | ascii-off | 2053.079 | 2151.064 | 2232.457 | 2232.457 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q164 | ascii-off | 6050.656 | 6140.541 | 6300.896 | 6300.896 | 4928 | 4928 | 4928 | 0/5 | ok |
| Q181 | ascii-off | 33999.897 | 34657.722 | 35570.721 | 35570.721 | 6 | 4928 | 8838 | 0/5 | ok |

## Index construction

- "ascii": build 0.750 s; parses 148486; resident 633 MiB; peak resident 675 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
