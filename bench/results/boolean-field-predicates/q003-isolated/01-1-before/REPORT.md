# Musenalm query checkpoint

Code revision: `c54afc3eb1cdc496734bfb30bcc9a3ea38988b78`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3645.968 | 3951.281 | 7762.617 | 2866 | 2948 | 2948 | 0/15 | 0/15 | ok |

## Index construction

- "ascii": build 0.905 s; snapshot write 0.472 s; parses 148486; resident 573 MiB; peak resident 736 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
