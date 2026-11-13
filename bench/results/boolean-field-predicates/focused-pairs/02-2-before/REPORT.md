# Musenalm query checkpoint

Code revision: `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5592.919 | 6211.128 | 7197.226 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | ascii-off | 3873.210 | 4941.123 | 6241.388 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 8685.393 | 12705.378 | 25032.678 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q028 | ascii-off | 2439.024 | 2533.357 | 2700.863 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | ascii-off | 2733.562 | 3003.499 | 3276.005 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | ascii-off | 13236.789 | 13979.606 | 16225.381 | 0 | 22185 | 22185 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 4207.027 | 4382.245 | 5231.401 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q001 | unicode-off | 6419.410 | 10196.486 | 20086.416 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3917.856 | 4191.076 | 4711.048 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 8826.636 | 13525.788 | 25310.880 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q028 | unicode-off | 2568.974 | 2862.613 | 3977.594 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | unicode-off | 2767.974 | 2921.740 | 3877.047 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | unicode-off | 13052.787 | 13730.892 | 14645.306 | 0 | 22570 | 22570 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 4199.632 | 4469.850 | 4530.444 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.969 s; snapshot write 0.515 s; parses 148486; resident 579 MiB; peak resident 744 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.021 s; snapshot write 0.445 s; parses 148486; resident 1088 MiB; peak resident 1219 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
