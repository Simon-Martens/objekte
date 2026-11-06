# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q113 | unicode-off | 2694.632 | 2979.424 | 4224.891 | 951 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q114 | unicode-off | 2949.543 | 4437.924 | 10904.318 | 3977 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "unicode": build 12.038 s; snapshot write 0.268 s; parses 148486; resident 257 MiB; peak resident 347 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.486/0.493/0.536 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
