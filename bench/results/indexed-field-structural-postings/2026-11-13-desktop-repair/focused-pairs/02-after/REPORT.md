# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14946.647 | 15265.061 | 19098.309 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3284.976 | 3952.418 | 10192.677 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6584.852 | 6648.837 | 8771.679 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6323.060 | 6797.311 | 7514.042 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2371.445 | 2863.433 | 8591.843 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2222.895 | 2482.639 | 3987.919 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 4958.452 | 5070.944 | 5870.494 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4435.041 | 4530.081 | 6946.274 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 22990.055 | 23919.239 | 24676.277 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4532.194 | 4680.003 | 5416.029 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4501.676 | 4608.782 | 6944.988 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 510.975 | 527.623 | 1784.527 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6153.176 | 6349.206 | 6859.791 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6627.239 | 6685.066 | 6806.687 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2659.489 | 3298.177 | 5732.715 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14950.523 | 15093.657 | 22066.689 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3413.751 | 3795.077 | 6843.085 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6530.379 | 6622.331 | 7735.640 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6363.498 | 6397.693 | 7069.635 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2470.397 | 2628.274 | 6110.209 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2272.654 | 2376.408 | 4061.203 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 4917.896 | 5241.884 | 6077.699 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4427.000 | 4522.038 | 5989.684 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 23080.373 | 23272.775 | 24477.959 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4330.344 | 4552.337 | 5145.204 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4787.176 | 5251.802 | 7213.616 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 541.470 | 555.374 | 1275.905 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6095.029 | 6214.642 | 6920.376 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6564.267 | 6644.232 | 6716.213 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2734.987 | 3067.244 | 8634.229 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.051 s; snapshot write 0.316 s; parses 148486; resident 492 MiB; peak resident 515 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.609/0.641/0.675 s over 5 runs.
- "unicode": build 0.947 s; snapshot write 0.306 s; parses 148486; resident 661 MiB; peak resident 661 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.610/0.628/0.674 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
