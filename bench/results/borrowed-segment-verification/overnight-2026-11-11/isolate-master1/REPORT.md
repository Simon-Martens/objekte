# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8439.314 | 9458.669 | 20571.293 | 1493 | 1504 | 1504 | 20/21 | 0/21 | ok |
| Q110 | ascii-off | 14662.103 | 17439.430 | 24602.466 | 4911 | 4928 | 4928 | 20/21 | 0/21 | ok |
| Q190 | ascii-off | 19508.147 | 22256.309 | 33242.497 | 3821 | 3821 | 3821 | 20/21 | 0/21 | ok |
| Q022 | unicode-warm | 8.315 | 11.060 | 29.585 | 1493 | 0 | 0 | 21/21 | 21/21 | ok |
| Q110 | unicode-warm | 6.151 | 6.602 | 28.203 | 4911 | 0 | 0 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 76543.619 | 81306.125 | 91011.527 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.564 s; snapshot write 0.446 s; parses 148486; resident 466 MiB; peak resident 550 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.322/1.338/1.416 s over 5 runs.
- "unicode": build 2.642 s; snapshot write 0.455 s; parses 148486; resident 693 MiB; peak resident 696 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.326/1.334/1.413 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
