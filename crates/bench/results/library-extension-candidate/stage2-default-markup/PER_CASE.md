# Final FTS per-case paired medians

Five alternating process pairs, seven measured rounds per case in each process. A positive delta means the candidate was slower. All listed candidate counts equal the reference counts in each pair.

| Cache MiB | Case | Query | Ref ms | Candidate ms | Paired delta ms | Delta % | Paired delta range ms |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 1 | annotation:fts ~ 'Goet' | 1.772 | 1.792 | +0.012 | +0.65 | -0.018 to +0.068 |
| 0 | 2 | annotation:fts ~ 'Schill' | 5.029 | 5.029 | +0.039 | +0.77 | -0.030 to +0.099 |
| 0 | 3 | annotation:fts ~ 'Mond' | 2.583 | 2.586 | +0.046 | +1.78 | -0.030 to +0.048 |
| 0 | 4 | annotation:fts ~ 'GOET' | 1.737 | 1.796 | +0.043 | +2.50 | -0.030 to +0.089 |
| 0 | 5 | annotation:fts ~ 'Liebe' | 3.276 | 3.320 | +0.013 | +0.41 | -0.044 to +0.115 |
| 0 | 6 | year = 1822 && annotation:fts ~ 'Goet' | 0.890 | 0.886 | +0.015 | +1.71 | -0.115 to +0.197 |
| 0 | 7 | year >= 1800 && annotation:fts ~ 'Wien' | 3.231 | 3.324 | +0.077 | +2.38 | -0.011 to +0.150 |
| 0 | 8 | year < 1800 && annotation:fts ~ 'Berlin' | 2.737 | 2.717 | +0.028 | +1.04 | -0.139 to +0.133 |
| 0 | 9 | annotation:fts ~ 'Heinrich' \|\| year = 1822 | 113.477 | 114.008 | +1.168 | +1.03 | +0.274 to +1.703 |
| 0 | 10 | annotation:fts !~ 'Goet' | 110.476 | 111.374 | +0.898 | +0.81 | +0.347 to +3.161 |
| 0 | 11 | preferred_title:fts ~ 'Goethe' | 1.627 | 1.622 | -0.005 | -0.32 | -0.132 to +0.104 |
| 0 | 12 | preferred_title:fts ~ 'Schiller' | 2.062 | 2.011 | -0.025 | -1.19 | -0.128 to +0.031 |
| 0 | 13 | preferred_title:fts ~ 'goethe' | 1.424 | 1.292 | -0.072 | -5.06 | -0.132 to -0.057 |
| 0 | 14 | preferred_title:fts ~ 'Gedicht' | 3.048 | 3.000 | -0.047 | -1.54 | -0.049 to -0.034 |
| 0 | 15 | preferred_title:fts ~ 'A%buch' | 4.865 | 4.826 | -0.043 | -0.89 | -0.124 to +0.041 |
| 0 | 16 | title_statement:fts ~ 'Verlag' | 2.255 | 2.290 | -0.026 | -1.17 | -0.043 to +0.104 |
| 0 | 17 | title_statement:fts ~ 'berlin' | 2.883 | 2.886 | -0.030 | -1.03 | -0.043 to +0.042 |
| 0 | 18 | contributors.annotation:fts ?~ 'Goet' | 0.958 | 0.964 | +0.006 | +0.62 | -0.021 to +0.038 |
| 0 | 19 | annotation:fts = 'Goethe' | 1.596 | 1.600 | +0.007 | +0.43 | -0.020 to +0.036 |
| 0 | 20 | annotation:fts ~ 'XYZunlikely' | 1.040 | 1.052 | +0.024 | +2.28 | -0.097 to +0.044 |
| 0 | 21 | Goethe | 1.536 | 1.324 | -0.133 | -8.64 | -0.291 to +0.283 |
| 0 | 22 | Schiller | 3.655 | 3.544 | -0.075 | -2.04 | -0.238 to +0.087 |
| 0 | 23 | Goethe | 0.535 | 0.532 | -0.012 | -2.16 | -0.014 to +0.008 |
| 0 | 24 | Liebe | 6.926 | 6.931 | -0.048 | -0.69 | -0.146 to +0.027 |
| 0 | 25 | Goethe | 5.706 | 5.444 | -0.262 | -4.59 | -0.390 to -0.191 |
| 0 | 26 | Goet.*he | 1.170 | 1.162 | -0.009 | -0.73 | -0.040 to -0.000 |
| 0 | 27 | Schill | 1.464 | 1.442 | -0.018 | -1.21 | -0.050 to +0.017 |
| 0 | 28 | \*\*[^*]+\*\* | 0.398 | 0.403 | +0.005 | +1.19 | -0.001 to +0.025 |
| 0 | 29 | <[^>]+> | 176.178 | 175.711 | +1.416 | +0.80 | -1.449 to +5.304 |
| 0 | 30 | Wien\|Berlin | 25.812 | 25.685 | -0.010 | -0.04 | -0.160 to +0.202 |
| 64 | 1 | annotation:fts ~ 'Goet' | 0.060 | 0.059 | +0.001 | +1.75 | -0.010 to +0.007 |
| 64 | 2 | annotation:fts ~ 'Schill' | 0.059 | 0.058 | -0.001 | -0.87 | -0.003 to +0.001 |
| 64 | 3 | annotation:fts ~ 'Mond' | 0.054 | 0.054 | +0.001 | +1.31 | -0.000 to +0.001 |
| 64 | 4 | annotation:fts ~ 'GOET' | 0.064 | 0.065 | +0.001 | +0.90 | -0.001 to +0.005 |
| 64 | 5 | annotation:fts ~ 'Liebe' | 0.056 | 0.057 | -0.001 | -1.09 | -0.010 to +0.002 |
| 64 | 6 | year = 1822 && annotation:fts ~ 'Goet' | 0.486 | 0.483 | -0.004 | -0.84 | -0.026 to -0.001 |
| 64 | 7 | year >= 1800 && annotation:fts ~ 'Wien' | 0.476 | 0.479 | -0.001 | -0.20 | -0.033 to +0.004 |
| 64 | 8 | year < 1800 && annotation:fts ~ 'Berlin' | 0.491 | 0.495 | +0.004 | +0.72 | -0.014 to +0.005 |
| 64 | 9 | annotation:fts ~ 'Heinrich' \|\| year = 1822 | 0.076 | 0.077 | -0.001 | -0.83 | -0.001 to +0.000 |
| 64 | 10 | annotation:fts !~ 'Goet' | 0.065 | 0.064 | -0.001 | -1.15 | -0.003 to +0.002 |
| 64 | 11 | preferred_title:fts ~ 'Goethe' | 0.059 | 0.058 | -0.001 | -1.31 | -0.001 to -0.000 |
| 64 | 12 | preferred_title:fts ~ 'Schiller' | 0.059 | 0.058 | -0.001 | -1.20 | -0.002 to +0.001 |
| 64 | 13 | preferred_title:fts ~ 'goethe' | 0.054 | 0.054 | -0.000 | -0.43 | -0.002 to +0.001 |
| 64 | 14 | preferred_title:fts ~ 'Gedicht' | 0.057 | 0.057 | +0.000 | +0.25 | -0.001 to +0.001 |
| 64 | 15 | preferred_title:fts ~ 'A%buch' | 0.025 | 0.026 | +0.000 | +0.79 | -0.000 to +0.001 |
| 64 | 16 | title_statement:fts ~ 'Verlag' | 0.057 | 0.058 | +0.001 | +2.07 | -0.000 to +0.003 |
| 64 | 17 | title_statement:fts ~ 'berlin' | 0.055 | 0.057 | +0.001 | +1.59 | -0.001 to +0.002 |
| 64 | 18 | contributors.annotation:fts ?~ 'Goet' | 0.027 | 0.027 | -0.000 | -1.28 | -0.001 to +0.001 |
| 64 | 19 | annotation:fts = 'Goethe' | 0.026 | 0.025 | -0.000 | -0.56 | -0.003 to +0.000 |
| 64 | 20 | annotation:fts ~ 'XYZunlikely' | 0.026 | 0.027 | +0.001 | +2.68 | -0.000 to +0.004 |
| 64 | 21 | Goethe | 0.216 | 0.220 | +0.002 | +0.91 | -0.009 to +0.010 |
| 64 | 22 | Schiller | 0.240 | 0.240 | +0.000 | +0.06 | -0.008 to +0.011 |
| 64 | 23 | Goethe | 0.196 | 0.202 | +0.006 | +3.07 | -0.002 to +0.010 |
| 64 | 24 | Liebe | 0.512 | 0.519 | +0.011 | +2.05 | +0.004 to +0.022 |
| 64 | 25 | Goethe | 4.005 | 3.660 | -0.349 | -8.71 | -0.387 to -0.214 |
| 64 | 26 | Goet.*he | 0.384 | 0.383 | +0.003 | +0.77 | -0.005 to +0.005 |
| 64 | 27 | Schill | 0.291 | 0.288 | -0.001 | -0.26 | -0.009 to +0.013 |
| 64 | 28 | \*\*[^*]+\*\* | 0.193 | 0.190 | -0.002 | -0.86 | -0.002 to +0.001 |
| 64 | 29 | <[^>]+> | 0.152 | 0.149 | +0.000 | +0.09 | -0.008 to +0.006 |
| 64 | 30 | Wien\|Berlin | 4.259 | 4.343 | +0.103 | +2.42 | +0.032 to +0.166 |
