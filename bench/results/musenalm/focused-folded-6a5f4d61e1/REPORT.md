# Musenalm query checkpoint

Code revision: `6a5f4d61e176c4278dcf03dcbcd1273347fa3fb9`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14392.073 | 18986.586 | 24213.889 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q070 | ascii-off | 107.806 | 165.699 | 211.182 | 1 | 1 | 1 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 8027.273 | 10545.945 | 15332.090 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 17104.420 | 19588.547 | 33745.971 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3800.303 | 4317.350 | 5762.678 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q115 | ascii-off | 4501.997 | 5728.805 | 9859.095 | 244 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 11394.944 | 15206.595 | 17633.952 | 3269 | 3424 | 3424 | 10/11 | 0/11 | ok |
| Q070 | unicode-off | 126.655 | 130.741 | 237.344 | 1 | 1 | 1 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 9368.907 | 10797.489 | 29705.584 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 14719.493 | 16438.256 | 26530.465 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 4749.611 | 5918.177 | 13868.249 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q115 | unicode-off | 3840.707 | 4442.613 | 6990.020 | 244 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 16.602 s; snapshot write 0.534 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.782/0.785/0.823 s over 5 runs.
- "unicode": build 17.973 s; snapshot write 0.490 s; parses 148486; resident 332 MiB; peak resident 403 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.896/0.920/0.974 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
