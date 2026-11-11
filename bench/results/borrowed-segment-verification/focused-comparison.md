# Focused verification comparison

All rows use four workers, zero verified-result cache, evidence enabled, and seven timed repetitions after one warm-up. Timings are milliseconds (median [minimum, maximum]). Allocation counts and allocated bytes are from a separate untimed query pass. The counts include file and output work; the in-memory boundary tests isolate text copying.

| Fixture | Query | Candidates | Hits | Source bytes read | Base ms | Final ms | Base allocations | Final allocations | Base allocated bytes | Final allocated bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| plain-json | value_identity | 1365 | 409 | 169205 | 1.242 [1.214, 1.371] | 1.547 [1.488, 1.763] | 54652 | 38272 | 4934303 | 4427261 |
| plain-json | value_folded | 1365 | 409 | 169205 | 1.259 [1.215, 1.396] | 1.255 [1.223, 1.347] | 54652 | 38272 | 4934303 | 4427261 |
| plain-json | source_identity | 1365 | 1365 | 169205 | 1.570 [1.536, 1.688] | 1.494 [1.452, 1.646] | 54880 | 34405 | 7015827 | 5881623 |
| plain-json | source_folded | 1365 | 1365 | 169205 | 1.600 [1.525, 2.315] | 1.472 [1.452, 1.603] | 54880 | 34405 | 7015827 | 5881623 |
| plain-json | source_filter | 1365 | 1365 | 169205 | 1.417 [1.380, 1.513] | 1.409 [1.395, 1.424] | 31692 | 24867 | 6431566 | 5672626 |
| plain-json | source_combined | 1365 | 1365 | 169205 | 1.584 [1.534, 1.638] | 1.483 [1.459, 1.724] | 58999 | 35794 | 7109986 | 5888422 |
| plain-json | scoped_duplicates | 1365 | 1365 | 169205 | 1.439 [1.412, 1.446] | 1.409 [1.387, 1.542] | 37151 | 30326 | 6828854 | 5971634 |
| plain-json | high_hit_logical | 1365 | 1365 | 169205 | 3.179 [2.942, 4.064] | 2.981 [2.890, 5.225] | 55232 | 38852 | 7033355 | 6526313 |
| escaped-decoded | value_identity | 1365 | 409 | 245645 | 1.495 [1.425, 1.825] | 2.293 [1.645, 2.761] | 76492 | 76492 | 6672631 | 6672631 |
| escaped-decoded | value_folded | 1365 | 409 | 245645 | 1.634 [1.612, 1.749] | 1.890 [1.844, 2.249] | 76492 | 76492 | 6672631 | 6672631 |
| escaped-decoded | source_identity | 1365 | 1365 | 245645 | 1.731 [1.712, 1.854] | 1.568 [1.540, 1.706] | 76720 | 26215 | 8754155 | 5283753 |
| escaped-decoded | source_folded | 1365 | 1365 | 245645 | 1.971 [1.904, 2.565] | 1.772 [1.656, 2.123] | 76720 | 26215 | 8754155 | 5283753 |
| escaped-decoded | source_filter | 1365 | 1365 | 245645 | 1.502 [1.486, 1.529] | 1.475 [1.460, 1.681] | 37152 | 24867 | 6690916 | 5757256 |
| escaped-decoded | source_combined | 1365 | 1365 | 245645 | 1.761 [1.737, 1.859] | 1.491 [1.473, 1.519] | 80839 | 30334 | 8848314 | 5918452 |
| escaped-decoded | scoped_duplicates | 1365 | 1365 | 245645 | 1.796 [1.760, 1.844] | 1.481 [1.452, 1.632] | 76736 | 30326 | 9358144 | 6056264 |
| escaped-decoded | high_hit_logical | 1365 | 1365 | 245645 | 3.651 [3.239, 4.740] | 3.502 [3.443, 4.762] | 77072 | 77072 | 8771683 | 8771683 |
| duplicates-decoded | value_identity | 512 | 102 | 2251752 | 7.337 [7.317, 7.961] | 7.592 [7.282, 8.175] | 956516 | 956516 | 64853389 | 64853389 |
| duplicates-decoded | value_folded | 512 | 102 | 2251752 | 7.677 [7.502, 7.860] | 7.952 [7.771, 8.109] | 956516 | 956516 | 64853389 | 64853389 |
| duplicates-decoded | source_identity | 512 | 512 | 2251752 | 7.687 [7.474, 7.794] | 1.724 [1.573, 2.172] | 956885 | 11561 | 65541611 | 4025587 |
| duplicates-decoded | source_folded | 512 | 512 | 2251752 | 8.072 [7.912, 8.342] | 1.725 [1.700, 1.739] | 956885 | 11561 | 65541611 | 4025587 |
| duplicates-decoded | source_filter | 512 | 341 | 2251752 | 0.849 [0.785, 1.068] | 1.103 [1.077, 1.337] | 11912 | 7816 | 4187005 | 3895337 |
| duplicates-decoded | source_combined | 512 | 341 | 2251752 | 7.650 [7.576, 7.949] | 1.613 [1.474, 1.675] | 957077 | 11753 | 65352559 | 4039287 |
| duplicates-decoded | scoped_duplicates | 512 | 341 | 2251752 | 6.463 [6.025, 6.858] | 1.048 [1.025, 1.167] | 952973 | 9185 | 39878699 | 3973115 |
| duplicates-decoded | high_hit_logical | 512 | 512 | 2251752 | 8.106 [7.907, 9.081] | 7.962 [7.856, 8.816] | 957022 | 957022 | 65546851 | 65546851 |
| field-duplicates-decoded | value_identity | 342 | 204 | 994536 | 3.565 [3.491, 4.164] | 3.545 [3.430, 5.341] | 289195 | 289195 | 28396517 | 28396517 |
| field-duplicates-decoded | value_folded | 342 | 204 | 994536 | 3.641 [3.594, 3.935] | 3.539 [3.508, 3.721] | 289195 | 289195 | 28396517 | 28396517 |
| field-duplicates-decoded | source_identity | 342 | 342 | 994536 | 3.626 [3.609, 3.690] | 0.767 [0.718, 0.826] | 288407 | 6599 | 28689715 | 2266111 |
| field-duplicates-decoded | source_folded | 342 | 342 | 994536 | 3.644 [3.597, 3.840] | 0.784 [0.777, 0.800] | 288407 | 6599 | 28689715 | 2266111 |
| field-duplicates-decoded | source_filter | 342 | 171 | 994536 | 0.769 [0.748, 0.778] | 0.506 [0.502, 0.513] | 50220 | 5076 | 2490831 | 1961757 |
| field-duplicates-decoded | source_combined | 342 | 171 | 994536 | 3.493 [3.488, 3.837] | 0.681 [0.674, 0.684] | 288088 | 6622 | 28308879 | 2022075 |
| field-duplicates-decoded | scoped_duplicates | 342 | 171 | 994536 | 3.599 [3.453, 3.773] | 0.513 [0.507, 0.872] | 287225 | 6101 | 24937387 | 2017231 |
| field-duplicates-decoded | high_hit_logical | 342 | 342 | 994536 | 3.606 [3.482, 5.238] | 3.620 [3.523, 4.450] | 288502 | 288502 | 28696011 | 28696011 |
| long-escaped-decoded | value_identity | 683 | 204 | 15449064 | 6.459 [6.173, 7.076] | 6.839 [6.367, 7.508] | 43775 | 43775 | 54902845 | 54902845 |
| long-escaped-decoded | value_folded | 683 | 204 | 15449064 | 26.557 [26.266, 27.176] | 27.069 [26.561, 28.361] | 43775 | 43775 | 54902845 | 54902845 |
| long-escaped-decoded | source_identity | 683 | 683 | 15449064 | 6.696 [6.454, 7.114] | 7.349 [7.265, 8.248] | 43873 | 13138 | 55942358 | 17980303 |
| long-escaped-decoded | source_folded | 683 | 683 | 15449064 | 26.522 [26.034, 26.954] | 26.931 [26.647, 27.304] | 43873 | 13138 | 55942358 | 17980303 |
| long-escaped-decoded | source_filter | 683 | 683 | 15449064 | 6.532 [6.242, 6.745] | 7.212 [6.843, 8.328] | 24083 | 12472 | 47944726 | 18217834 |
| long-escaped-decoded | source_combined | 683 | 683 | 15449064 | 6.529 [6.437, 6.750] | 7.409 [6.979, 8.263] | 45946 | 15211 | 55990141 | 18298554 |
| long-escaped-decoded | scoped_duplicates | 683 | 683 | 15449064 | 6.513 [6.384, 6.635] | 7.169 [6.972, 7.253] | 43889 | 15203 | 56246267 | 18368166 |
| long-escaped-decoded | high_hit_logical | 683 | 683 | 15449064 | 7.767 [7.570, 8.566] | 8.858 [7.484, 9.904] | 44054 | 44054 | 55951694 | 55951694 |
