# Musenalm query checkpoint

Code revision: `012f6fd5188c0298a0330c2293d062d3f27feaf4`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Status |
|---|---|---:|---:|---:|---:|---:|---:|---|
| Q002 | ascii-off | 184391.431 | 185285.022 | 192148.091 | 5608 | 6645 | 6645 | ok |
| Q006 | ascii-off | 2447393.532 | 2465298.644 | 2514998.633 | 65548 | 148486 | 148486 | ok |
| Q007 | ascii-off | 2572588.126 | 2579109.627 | 2600286.628 | 147798 | 148486 | 148486 | ok |
| Q013 | ascii-off | 244275.786 | 244523.423 | 246956.236 | 57 | 2143 | 2143 | ok |
| Q024 | ascii-off | 70720.398 | 72028.273 | 72863.511 | 1927 | 4636 | 4636 | ok |

## Index construction

- "ascii": build 14.329 s; snapshot write 0.259 s; parses 148486; resident 241 MiB; estimated index 65 MiB; fresh-process restore median 0.530 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
