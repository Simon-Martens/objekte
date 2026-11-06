# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q113 | unicode-off | 2698.737 | 2839.764 | 4091.675 | 951 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q114 | unicode-off | 2938.604 | 4688.324 | 11597.969 | 3977 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "unicode": build 12.038 s; snapshot write 0.284 s; parses 148486; resident 257 MiB; peak resident 347 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.492/0.500/0.550 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
