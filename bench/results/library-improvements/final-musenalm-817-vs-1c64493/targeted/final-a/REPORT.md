# Musenalm query checkpoint

Code revision: `98410748d0b04b0d5d93b2eb36fb85e2e4fb7439`. Repetitions: 10. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q074 | ascii-off | 4962.816 | 6842.573 | 11308.814 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | ascii-off | 148500.682 | 173207.631 | 194217.694 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | ascii-off | 153418.123 | 164290.252 | 174841.969 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | ascii-off | 4567.431 | 8001.954 | 10788.929 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | ascii-off | 263210.656 | 280657.702 | 319428.639 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q074 | unicode-off | 4567.750 | 6308.169 | 10966.134 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | unicode-off | 135482.912 | 172405.082 | 197247.379 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | unicode-off | 139005.652 | 156520.789 | 193626.204 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | unicode-off | 4982.457 | 9637.741 | 21621.973 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | unicode-off | 264825.566 | 282480.357 | 314472.825 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |

## Index construction

- "ascii": build 1.170 s; snapshot write 0.199 s; parses 148486; resident 484 MiB; peak resident 576 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.541/0.545/0.557 s over 5 runs.
- "unicode": build 1.264 s; snapshot write 0.199 s; parses 148486; resident 720 MiB; peak resident 789 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.545/0.549/0.555 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
