# Musenalm query checkpoint

Code revision: `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q004 | ascii-off | 7748.794 | 9968.090 | 25581.225 | 2995 | 7766 | 7766 | 0/15 | 0/15 | ok |

## Index construction

- "ascii": build 0.934 s; snapshot write 0.506 s; parses 148486; resident 563 MiB; peak resident 726 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
