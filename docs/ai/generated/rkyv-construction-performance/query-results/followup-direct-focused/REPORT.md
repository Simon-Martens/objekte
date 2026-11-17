# Musenalm query checkpoint

Code revision: `697e0f5274ce8697da6343c0622e460f69eb70cc`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12595.326 | 15309.292 | 87393.702 | 87393.702 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 5014.727 | 6524.738 | 8366.163 | 8366.163 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 13062.541 | 15550.663 | 83358.686 | 83358.686 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 7148.818 | 7166.022 | 9046.440 | 9046.440 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.855 s; parses 148486; resident 719 MiB; peak resident 782 MiB; estimated index 0 MiB; rkyv archive 141 MiB.
- "unicode": build 0.841 s; parses 148486; resident 978 MiB; peak resident 996 MiB; estimated index 0 MiB; rkyv archive 141 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
