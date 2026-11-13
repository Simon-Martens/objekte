# Musenalm query checkpoint

Code revision: `a7a31feb5da625008c8e0e979de32bdcb64a80cd`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q004 | ascii-off | 7686.631 | 9098.824 | 19476.195 | 2995 | 7766 | 7766 | 0/15 | 0/15 | ok |

## Index construction

- "ascii": build 0.947 s; snapshot write 0.473 s; parses 148486; resident 570 MiB; peak resident 734 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
