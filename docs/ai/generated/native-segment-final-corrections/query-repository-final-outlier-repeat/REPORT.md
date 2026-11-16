# Musenalm query checkpoint

Code revision: `170eaeca03efa4cbc591a43fc3e611dd333b6ad2`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q068 | ascii-off | 5216.909 | 5379.914 | 8373.159 | 8373.159 | 1 | 4010 | 0 | 0/3 | ok |
| Q069 | ascii-off | 6212.982 | 6340.062 | 7673.343 | 7673.343 | 4010 | 4010 | 0 | 0/3 | ok |
| Q077 | ascii-off | 4457.296 | 5095.948 | 6028.029 | 6028.029 | 818 | 4636 | 0 | 0/3 | ok |
| Q119 | ascii-off | 4573.697 | 4615.766 | 5030.566 | 5030.566 | 133948 | 133948 | 0 | 0/3 | ok |
| Q068 | unicode-off | 6190.827 | 6926.392 | 8449.210 | 8449.210 | 1 | 4010 | 0 | 0/3 | ok |
| Q069 | unicode-off | 7278.589 | 7348.913 | 7704.129 | 7704.129 | 4010 | 4010 | 0 | 0/3 | ok |
| Q077 | unicode-off | 2196.055 | 2388.989 | 3503.693 | 3503.693 | 818 | 2011 | 0 | 0/3 | ok |
| Q119 | unicode-off | 4588.050 | 4614.644 | 5084.742 | 5084.742 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.643 s; parses 148486; resident 598 MiB; peak resident 639 MiB; estimated index 0 MiB.
- "unicode": build 0.692 s; parses 148486; resident 717 MiB; peak resident 735 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
