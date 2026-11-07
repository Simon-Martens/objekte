# Targeted alternating cache-off retime

Each process used the same copied Musenalm fixture, five cases, ASCII and Unicode cache off, 32 query workers, and ten timed repetitions per case. Process order was baseline, final, baseline, final. Each cell is the median in milliseconds; delta is final minus its adjacent baseline. Source/candidate work and answers remained equal.

| Case | Profile | Baseline A | Final A | Delta A | Baseline B | Final B | Delta B |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q074 | ascii-off | 4.877 | 6.843 | +1.966 | 6.031 | 6.741 | +0.710 |
| Q074 | unicode-off | 6.492 | 6.308 | -0.183 | 5.983 | 6.219 | +0.237 |
| Q116 | ascii-off | 162.135 | 173.208 | +11.073 | 159.477 | 165.012 | +5.534 |
| Q116 | unicode-off | 157.450 | 172.405 | +14.955 | 173.383 | 169.248 | -4.135 |
| Q117 | ascii-off | 151.286 | 164.290 | +13.004 | 162.118 | 158.342 | -3.776 |
| Q117 | unicode-off | 152.723 | 156.521 | +3.798 | 148.407 | 158.482 | +10.075 |
| Q158 | ascii-off | 9.609 | 8.002 | -1.607 | 9.215 | 8.720 | -0.495 |
| Q158 | unicode-off | 9.663 | 9.638 | -0.026 | 8.689 | 10.073 | +1.384 |
| Q187 | ascii-off | 284.485 | 280.658 | -3.827 | 286.747 | 292.495 | +5.748 |
| Q187 | unicode-off | 286.761 | 282.480 | -4.281 | 300.046 | 287.107 | -12.939 |

The large Unicode broad-scan slowdowns from the full run do not repeat in both paired blocks. Q117 remains slightly slower but its sample ranges overlap. Q187 becomes faster in both Unicode blocks. Q116 changes direction between blocks. Q074 ASCII has a small variable increase with overlapping sample ranges. No specific extra source work or code overhead is established by these samples.
