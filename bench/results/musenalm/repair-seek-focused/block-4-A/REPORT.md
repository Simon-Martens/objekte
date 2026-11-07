# Musenalm query checkpoint

Code revision: `6e1e96d9ead1c32f30f2a4f1d42afbe100f9edb4`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 196533.689 | 231981.644 | 238634.025 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 89129.537 | 94047.060 | 128366.921 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 3225.347 | 6921.954 | 10941.346 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q116 | ascii-off | 101810.028 | 115323.890 | 135332.042 | 70428 | 133948 | 133948 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.108 s; snapshot write 0.264 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.596/0.612/0.650 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
