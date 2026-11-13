# Musenalm query checkpoint

Code revision: `a7a31feb5da625008c8e0e979de32bdcb64a80cd`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5481.348 | 6003.457 | 7316.759 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | ascii-off | 3642.964 | 3948.574 | 4528.498 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 10880.201 | 13458.052 | 25666.797 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q028 | ascii-off | 2414.547 | 2584.359 | 2951.277 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | ascii-off | 2748.290 | 2878.292 | 3409.666 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | ascii-off | 13234.628 | 14352.410 | 15523.201 | 0 | 22185 | 22185 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 4123.169 | 4180.439 | 4703.695 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q001 | unicode-off | 5553.627 | 6322.135 | 10855.706 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3510.422 | 3574.987 | 4774.031 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 11199.034 | 12116.548 | 22530.998 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q028 | unicode-off | 2573.620 | 2716.007 | 3669.371 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | unicode-off | 2724.430 | 2796.795 | 3254.026 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | unicode-off | 13641.015 | 14441.651 | 15271.467 | 0 | 22570 | 22570 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 4466.837 | 4545.626 | 5820.181 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.968 s; snapshot write 0.497 s; parses 148486; resident 585 MiB; peak resident 750 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 0.988 s; snapshot write 0.486 s; parses 148486; resident 1108 MiB; peak resident 1238 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
