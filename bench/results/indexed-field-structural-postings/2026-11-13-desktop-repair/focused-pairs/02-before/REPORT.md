# Musenalm query checkpoint

Code revision: `4cf5d759b9b1905a2063030e394969b05581763d`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 14570.777 | 14922.217 | 16257.493 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | ascii-off | 3507.232 | 3856.585 | 6770.495 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 6571.041 | 6683.464 | 8932.508 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | ascii-off | 6277.886 | 6312.454 | 7022.722 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | ascii-off | 2285.300 | 2694.865 | 7177.762 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | ascii-off | 2053.150 | 2280.646 | 4039.287 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | ascii-off | 4812.294 | 5138.632 | 6155.149 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | ascii-off | 4488.587 | 4824.221 | 6337.829 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 22837.197 | 23164.573 | 29321.073 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 4357.447 | 4518.723 | 5179.119 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4443.132 | 4823.633 | 7345.773 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 529.522 | 544.557 | 1169.426 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | ascii-off | 6233.665 | 6331.367 | 7009.563 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | ascii-off | 6825.817 | 7305.837 | 8305.353 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 2873.914 | 3034.926 | 8450.201 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q001 | unicode-off | 14905.301 | 16015.557 | 19249.397 | 3269 | 4872 | 4872 | 6/7 | 0/7 | ok |
| Q052 | unicode-off | 3385.906 | 3735.961 | 6882.042 | 86764 | 86764 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 6542.791 | 6671.122 | 8067.486 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q112 | unicode-off | 6260.983 | 6553.129 | 6968.344 | 25 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 2276.443 | 2776.081 | 8924.144 | 70428 | 70428 | 0 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 2198.120 | 2563.068 | 5137.395 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q167 | unicode-off | 4829.221 | 5295.706 | 6144.555 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q168 | unicode-off | 4415.456 | 4524.728 | 5676.919 | 350 | 1202 | 2385 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 23005.500 | 23706.782 | 25141.045 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 4576.049 | 4617.930 | 5233.059 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4689.749 | 4810.781 | 6748.382 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 581.582 | 621.804 | 1555.227 | 7 | 203 | 203 | 7/7 | 0/7 | ok |
| Q195 | unicode-off | 6180.140 | 6470.127 | 8114.227 | 0 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q196 | unicode-off | 6676.738 | 6743.129 | 7021.559 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2750.698 | 3230.947 | 5829.170 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.033 s; snapshot write 0.300 s; parses 148486; resident 488 MiB; peak resident 510 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.606/0.611/0.685 s over 5 runs.
- "unicode": build 0.999 s; snapshot write 0.310 s; parses 148486; resident 684 MiB; peak resident 684 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.611/0.625/0.666 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
