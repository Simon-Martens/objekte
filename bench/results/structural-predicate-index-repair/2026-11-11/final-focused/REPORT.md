# Musenalm query checkpoint

Code revision: `90b52907c7f9c9364305e38e1b689c9766cc84d0`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q115 | unicode-off | 100.429 | 108.423 | 270.638 | 244 | 244 | 0 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 8339.248 | 10050.852 | 11443.297 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 9139.471 | 18109.083 | 20875.387 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q118 | unicode-off | 686.539 | 698.601 | 822.144 | 254 | 254 | 0 | 6/7 | 0/7 | ok |
| Q120 | unicode-off | 9278.933 | 9672.131 | 11209.889 | 42700 | 42700 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "unicode": build 2.672 s; snapshot write 0.446 s; parses 148486; resident 548 MiB; peak resident 574 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.313/1.319/1.401 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
