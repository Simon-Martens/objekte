# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 100. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8761.759 | 10194.711 | 17555.008 | 1493 | 1504 | 1504 | 99/100 | 0/100 | ok |
| Q110 | ascii-off | 14560.001 | 16962.064 | 25673.057 | 4911 | 4928 | 4928 | 99/100 | 0/100 | ok |

## Index construction

- "ascii": build 2.568 s; snapshot write 0.433 s; parses 148486; resident 462 MiB; peak resident 538 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.322/1.330/1.413 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
