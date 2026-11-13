# Musenalm query checkpoint

Code revision: `a7a31feb5da625008c8e0e979de32bdcb64a80cd`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5515.447 | 6062.995 | 9641.176 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | ascii-off | 3546.057 | 4078.535 | 4547.090 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 10940.850 | 14133.098 | 24224.348 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q028 | ascii-off | 2430.452 | 2707.100 | 3074.679 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | ascii-off | 2735.075 | 2938.923 | 3216.937 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | ascii-off | 13909.007 | 14339.734 | 16268.460 | 0 | 22185 | 22185 | 0/7 | 0/7 | ok |
| Q190 | ascii-off | 4112.593 | 4273.993 | 4815.679 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |
| Q001 | unicode-off | 5703.009 | 6423.224 | 8555.762 | 3269 | 4872 | 4872 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3515.913 | 3780.224 | 4731.210 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 12326.565 | 13855.188 | 28308.965 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q028 | unicode-off | 2444.126 | 2593.340 | 3259.453 | 4177 | 4177 | 4177 | 0/7 | 0/7 | ok |
| Q069 | unicode-off | 2748.189 | 3068.291 | 4061.624 | 4010 | 4010 | 4010 | 0/7 | 0/7 | ok |
| Q100 | unicode-off | 13373.713 | 13931.441 | 16069.899 | 0 | 22570 | 22570 | 0/7 | 0/7 | ok |
| Q190 | unicode-off | 4183.161 | 4526.782 | 4705.442 | 3821 | 3821 | 3821 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.961 s; snapshot write 0.473 s; parses 148486; resident 578 MiB; peak resident 743 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.013 s; snapshot write 0.477 s; parses 148486; resident 1096 MiB; peak resident 1226 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
