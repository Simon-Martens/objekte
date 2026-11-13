# Musenalm query checkpoint

Code revision: `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q214 | ascii-off | 2667.716 | 3202.154 | 5781.229 | 133948 | 133948 | 0 | 14/15 | 0/15 | ok |

## Index construction

- "ascii": build 1.031 s; snapshot write 0.263 s; parses 148486; resident 494 MiB; peak resident 517 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.620/0.657/0.708 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
