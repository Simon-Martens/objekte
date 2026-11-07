# Musenalm query checkpoint

Code revision: `98410748d0b04b0d5d93b2eb36fb85e2e4fb7439`. Repetitions: 10. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q074 | ascii-off | 5036.753 | 6740.686 | 10149.044 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | ascii-off | 134300.304 | 165011.723 | 188708.993 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | ascii-off | 138291.762 | 158341.623 | 180822.420 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | ascii-off | 5216.624 | 8720.341 | 19050.176 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | ascii-off | 271375.487 | 292495.376 | 304514.585 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q074 | unicode-off | 5431.763 | 6219.444 | 11255.593 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | unicode-off | 131788.482 | 169248.420 | 217281.003 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | unicode-off | 143716.559 | 158481.508 | 192466.880 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | unicode-off | 5406.304 | 10072.522 | 17665.041 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | unicode-off | 269522.663 | 287106.583 | 310252.019 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |

## Index construction

- "ascii": build 1.153 s; snapshot write 0.203 s; parses 148486; resident 533 MiB; peak resident 624 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.539/0.541/0.548 s over 5 runs.
- "unicode": build 1.236 s; snapshot write 0.210 s; parses 148486; resident 803 MiB; peak resident 873 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.541/0.550/0.554 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
