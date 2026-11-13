# Musenalm query checkpoint

Code revision: `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q214 | ascii-off | 2943.816 | 3360.510 | 6268.693 | 133948 | 133948 | 0 | 14/15 | 0/15 | ok |

## Index construction

- "ascii": build 1.021 s; snapshot write 0.259 s; parses 148486; resident 487 MiB; peak resident 510 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.626/0.650/0.683 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
