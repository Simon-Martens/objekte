# Musenalm query checkpoint

Code revision: `95f1736d448a01d94650afd237149c158c0e7a8a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 16607.874 | 17762.422 | 18605.885 | 2866 | 2948 | 2948 | 6/7 | 0/7 | ok |
| Q072 | ascii-off | 8384.894 | 9353.673 | 13558.329 | 1287 | 2251 | 2251 | 6/7 | 0/7 | ok |
| Q073 | ascii-off | 2429.732 | 2573.062 | 3187.856 | 225 | 893 | 893 | 6/7 | 0/7 | ok |
| Q076 | ascii-off | 32279.621 | 33337.176 | 51326.354 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q119 | ascii-off | 25949.484 | 38349.647 | 52326.281 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | ascii-off | 7491.136 | 7977.399 | 8780.817 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q144 | ascii-off | 39559.550 | 47584.738 | 61678.812 | 3939 | 8570 | 8570 | 6/7 | 0/7 | ok |
| Q159 | ascii-off | 16557.159 | 18845.365 | 26786.226 | 5 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 45827.959 | 51697.801 | 70994.484 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 16276.673 | 17555.474 | 36000.928 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q180 | ascii-off | 2867.845 | 2990.685 | 3599.739 | 18 | 1202 | 1269 | 6/7 | 0/7 | ok |
| Q187 | ascii-off | 453359.489 | 458607.103 | 461367.134 | 133948 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q190 | ascii-off | 19473.334 | 20989.562 | 30581.472 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |
| Q003 | unicode-off | 16656.185 | 17322.165 | 19611.603 | 2866 | 2948 | 2948 | 6/7 | 0/7 | ok |
| Q072 | unicode-off | 8353.526 | 9178.394 | 14325.118 | 1287 | 2251 | 2251 | 6/7 | 0/7 | ok |
| Q073 | unicode-off | 2487.180 | 2607.716 | 3510.882 | 225 | 893 | 893 | 6/7 | 0/7 | ok |
| Q076 | unicode-off | 32293.096 | 33383.082 | 48374.922 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q119 | unicode-off | 26967.186 | 36514.101 | 41460.770 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 7567.689 | 8565.122 | 9175.339 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q144 | unicode-off | 38054.273 | 42607.443 | 56966.073 | 3939 | 8570 | 8570 | 6/7 | 0/7 | ok |
| Q159 | unicode-off | 15666.426 | 15907.980 | 25727.918 | 5 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 44588.392 | 49748.170 | 62506.346 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 16383.793 | 17024.927 | 30863.662 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q180 | unicode-off | 2989.884 | 3812.999 | 5011.550 | 18 | 1202 | 1269 | 6/7 | 0/7 | ok |
| Q187 | unicode-off | 443014.615 | 443963.607 | 449677.626 | 133948 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q190 | unicode-off | 18818.886 | 19410.617 | 28680.232 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.569 s; snapshot write 0.431 s; parses 148486; resident 534 MiB; peak resident 560 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.315/1.324/1.369 s over 5 runs.
- "unicode": build 2.602 s; snapshot write 0.443 s; parses 148486; resident 737 MiB; peak resident 737 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.341/1.348/1.429 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
