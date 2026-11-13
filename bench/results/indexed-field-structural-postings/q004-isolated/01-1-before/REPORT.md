# Musenalm query checkpoint

Code revision: `7c8ce508d54687afc5eeced93611b5662266b74a`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q004 | unicode-off | 7870.869 | 9216.114 | 25345.864 | 2995 | 7773 | 7773 | 0/15 | 0/15 | ok |

## Index construction

- "unicode": build 0.972 s; snapshot write 0.489 s; parses 148486; resident 592 MiB; peak resident 759 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
