# Musenalm query checkpoint

Code revision: `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-off | 20017.677 | 23096.842 | 62831.983 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |
| Q190 | unicode-off | 20298.845 | 22553.772 | 64015.395 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.573 s; snapshot write 0.532 s; parses 148486; resident 466 MiB; peak resident 547 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.330/1.334/1.427 s over 5 runs.
- "unicode": build 2.568 s; snapshot write 0.542 s; parses 148486; resident 699 MiB; peak resident 706 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.334/1.340/1.438 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
