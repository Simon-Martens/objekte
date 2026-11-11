# Musenalm query checkpoint

Code revision: `49ac3f503ac63f8303f85122a1ec4f0043949cda`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | unicode-off | 6513.324 | 7325.791 | 10783.341 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q186 | unicode-off | 4632.318 | 5030.517 | 8473.609 | 627 | 1627 | 1627 | 20/21 | 0/21 | ok |

## Index construction

- "unicode": build 2.727 s; snapshot write 0.448 s; parses 148486; resident 471 MiB; peak resident 545 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.356/1.366/1.464 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
