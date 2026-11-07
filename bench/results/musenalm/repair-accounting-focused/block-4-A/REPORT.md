# Musenalm query checkpoint

Code revision: `c5787c1d8099a2ac99fe9157035451d41ccd4015`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 201869.275 | 237987.204 | 257888.332 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 84483.271 | 129713.344 | 151315.386 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 4071.650 | 10944.642 | 20001.306 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q116 | ascii-off | 110881.779 | 123246.994 | 156757.445 | 70428 | 133948 | 133948 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.672 s; snapshot write 0.308 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.596/0.616/0.658 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
