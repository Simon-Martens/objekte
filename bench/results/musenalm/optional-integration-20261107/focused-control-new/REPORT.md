# Musenalm query checkpoint

Code revision: `ca99218530e60d76bfd6402ea0b0528613430f28`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 335576.696 | 359348.266 | 396368.194 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | ascii-off | 397192.047 | 481592.646 | 561557.799 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q006 | unicode-off | 336506.469 | 364953.918 | 392194.348 | 65548 | 148486 | 148486 | 4/5 | 0/5 | ok |
| Q007 | unicode-off | 384629.932 | 499706.753 | 528369.548 | 147798 | 148486 | 148486 | 4/5 | 0/5 | ok |

## Index construction

- "ascii": build 1.235 s; snapshot write 0.438 s; parses 148486; resident 539 MiB; peak resident 629 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.543/0.545/0.574 s over 5 runs.
- "unicode": build 1.334 s; snapshot write 0.378 s; parses 148486; resident 714 MiB; peak resident 783 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.539/0.545/0.557 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
