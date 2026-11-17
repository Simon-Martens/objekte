# Musenalm query checkpoint

Code revision: `697e0f5274ce8697da6343c0622e460f69eb70cc`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12649.901 | 14049.524 | 67849.392 | 67849.392 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 4944.164 | 4959.687 | 5747.984 | 5747.984 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 14033.082 | 15062.028 | 81714.288 | 81714.288 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 7260.672 | 7413.709 | 9766.811 | 9766.811 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": open/build 0.804 s; parses 148486; resident 762 MiB; peak resident 791 MiB; estimated index 120 MiB; rkyv archive 69 MiB.
- "unicode": open/build 0.889 s; parses 148486; resident 1212 MiB; peak resident 1230 MiB; estimated index 124 MiB; rkyv archive 69 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
