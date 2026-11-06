# Musenalm query checkpoint

Code revision: `6e1e96d9ead1c32f30f2a4f1d42afbe100f9edb4`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 187307.850 | 226403.309 | 235197.039 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 82530.325 | 92849.025 | 127726.931 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 3952.440 | 8680.212 | 27094.574 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q116 | ascii-off | 92571.650 | 120194.789 | 129774.446 | 70428 | 133948 | 133948 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 10.834 s; snapshot write 0.448 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.585/0.588/0.673 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
