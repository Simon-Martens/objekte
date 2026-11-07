# Musenalm query checkpoint

Code revision: `ef44e42e566abe1a808094cd7933cc798a63f63d`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 242690.230 | 303137.371 | 345407.402 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | ascii-off | 363901.836 | 387689.430 | 397331.762 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q006 | unicode-off | 283051.252 | 289837.365 | 297329.039 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | unicode-off | 338494.366 | 343787.808 | 349456.126 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |

## Index construction

- "ascii": build 1.230 s; snapshot write 0.553 s; parses 148486; resident 496 MiB; peak resident 588 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.534/0.551/0.563 s over 5 runs.
- "unicode": build 1.273 s; snapshot write 0.511 s; parses 148486; resident 714 MiB; peak resident 784 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.538/0.550/0.559 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
