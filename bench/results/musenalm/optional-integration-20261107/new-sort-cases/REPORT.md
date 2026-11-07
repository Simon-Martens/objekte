# Musenalm query checkpoint

Code revision: `c6160c2d9a68bc1d155dd4157aa5007497a24687`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q189 | ascii-off | 3883.262 | 4092.128 | 4706.594 | 7 | 203 | 203 | 4/5 | 0/5 | ok |
| Q190 | ascii-off | 76908.724 | 79364.577 | 85818.567 | 3821 | 3821 | 3821 | 4/5 | 0/5 | ok |
| Q191 | ascii-off | 1695294.225 | 1748186.638 | 1800139.306 | 95153 | 95153 | 96921 | 4/5 | 0/5 | ok |
| Q192 | ascii-off | 1224803.166 | 1252783.075 | 1276276.720 | 70386 | 70390 | 70390 | 4/5 | 0/5 | ok |
| Q193 | ascii-off | 13050.474 | 14606.755 | 15802.992 | 18 | 1202 | 1311 | 4/5 | 0/5 | ok |
| Q194 | ascii-off | 284.085 | 314.916 | 536.052 | 7 | 203 | 203 | 5/5 | 0/5 | ok |
| Q189 | ascii-warm | 110.782 | 114.126 | 165.383 | 7 | 0 | 7 | 5/5 | 5/5 | ok |
| Q190 | ascii-warm | 52399.804 | 52864.810 | 66537.507 | 3821 | 0 | 3821 | 5/5 | 5/5 | ok |
| Q191 | ascii-warm | 952880.564 | 965194.071 | 1003407.813 | 95153 | 0 | 96921 | 5/5 | 5/5 | ok |
| Q192 | ascii-warm | 639790.204 | 660831.568 | 669532.199 | 70386 | 0 | 70386 | 5/5 | 5/5 | ok |
| Q193 | ascii-warm | 762.430 | 770.786 | 1426.938 | 18 | 0 | 60 | 5/5 | 5/5 | ok |
| Q194 | ascii-warm | 2.089 | 2.906 | 8.025 | 7 | 0 | 0 | 5/5 | 5/5 | ok |
| Q189 | unicode-off | 3865.189 | 5214.705 | 6982.318 | 7 | 203 | 203 | 4/5 | 0/5 | ok |
| Q190 | unicode-off | 77314.933 | 81224.944 | 102350.955 | 3821 | 3821 | 3821 | 4/5 | 0/5 | ok |
| Q191 | unicode-off | 1737451.491 | 1756752.841 | 1894585.151 | 95153 | 95153 | 96921 | 4/5 | 0/5 | ok |
| Q192 | unicode-off | 1201831.983 | 1298536.332 | 1346746.067 | 70386 | 70390 | 70390 | 4/5 | 0/5 | ok |
| Q193 | unicode-off | 14115.714 | 16580.746 | 34758.372 | 18 | 1202 | 1311 | 4/5 | 0/5 | ok |
| Q194 | unicode-off | 248.722 | 291.313 | 410.695 | 7 | 203 | 203 | 5/5 | 0/5 | ok |
| Q189 | unicode-warm | 114.410 | 116.409 | 171.610 | 7 | 0 | 7 | 5/5 | 5/5 | ok |
| Q190 | unicode-warm | 53139.182 | 54493.899 | 63090.425 | 3821 | 0 | 3821 | 5/5 | 5/5 | ok |
| Q191 | unicode-warm | 952688.704 | 957605.250 | 982774.864 | 95153 | 0 | 96921 | 5/5 | 5/5 | ok |
| Q192 | unicode-warm | 662155.834 | 665424.744 | 673425.086 | 70386 | 0 | 70386 | 5/5 | 5/5 | ok |
| Q193 | unicode-warm | 755.935 | 788.224 | 965.357 | 18 | 0 | 60 | 5/5 | 5/5 | ok |
| Q194 | unicode-warm | 2.343 | 3.141 | 16.223 | 7 | 0 | 0 | 5/5 | 5/5 | ok |

## Index construction

- "ascii": build 1.253 s; snapshot write 0.352 s; parses 148486; resident 512 MiB; peak resident 603 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.552/0.556/0.567 s over 5 runs.
- "unicode": build 1.296 s; snapshot write 0.344 s; parses 148486; resident 723 MiB; peak resident 792 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.530/0.558/0.566 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
