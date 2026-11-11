# Musenalm query checkpoint

Code revision: `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-off | 19567.331 | 25067.434 | 61504.077 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |
| Q190 | unicode-off | 18946.655 | 22520.870 | 56263.051 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.561 s; snapshot write 0.545 s; parses 148486; resident 510 MiB; peak resident 549 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.332/1.338/1.457 s over 5 runs.
- "unicode": build 2.597 s; snapshot write 0.566 s; parses 148486; resident 688 MiB; peak resident 690 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.343/1.348/1.551 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
