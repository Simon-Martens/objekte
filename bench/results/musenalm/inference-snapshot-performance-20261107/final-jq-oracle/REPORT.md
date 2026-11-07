# Independent jq membership oracle

New-core code revision: `8ef91ea94de6d636d47ad79adfb2bbe7021cf09a`. jq: `jq-1.8.1`.
Source files: 148486; fixture metadata fingerprint: `eec50dbfbfd023aad31e9c3609f4a7f14b177ecc1a022c182321ae704c188cf6`.
Complete ID-set comparisons: 300/300 passed.

jq evaluated handwritten predicates directly over each original JSON file.
The new core separately returned complete membership sets under ASCII and Unicode.
This oracle does not use the new core's parser, binder, scanner, index, or cache.
The manifest contains SHA-256 digests of every complete per-case membership set.
Pass `--save-membership` to retain the full expected and actual ID sets as gzip JSON.

| Case | jq matches | ASCII | Unicode |
|---|---:|---|---|
| Q001 | 3269 | ok | ok |
| Q002 | 5608 | ok | ok |
| Q003 | 2866 | ok | ok |
| Q004 | 2995 | ok | ok |
| Q005 | 516 | ok | ok |
| Q006 | 65548 | ok | ok |
| Q007 | 147798 | ok | ok |
| Q008 | 0 | ok | ok |
| Q009 | 1 | ok | ok |
| Q010 | 1 | ok | ok |
| Q011 | 62 | ok | ok |
| Q012 | 31 | ok | ok |
| Q013 | 57 | ok | ok |
| Q014 | 1742 | ok | ok |
| Q015 | 2684 | ok | ok |
| Q016 | 325 | ok | ok |
| Q017 | 390 | ok | ok |
| Q018 | 228 | ok | ok |
| Q019 | 1316 | ok | ok |
| Q020 | 163 | ok | ok |
| Q021 | 230 | ok | ok |
| Q022 | 1493 | ok | ok |
| Q023 | 5307 | ok | ok |
| Q024 | 1927 | ok | ok |
| Q025 | 60319 | ok | ok |
| Q026 | 18825 | ok | ok |
| Q027 | 215 | ok | ok |
| Q028 | 4177 | ok | ok |
| Q029 | 7 | ok | ok |
| Q030 | 1264 | ok | ok |
| Q031 | 203 | ok | ok |
| Q032 | 7 | ok | ok |
| Q033 | 210 | ok | ok |
| Q034 | 1 | ok | ok |
| Q035 | 124 | ok | ok |
| Q036 | 86 | ok | ok |
| Q037 | 753 | ok | ok |
| Q038 | 3257 | ok | ok |
| Q039 | 0 | ok | ok |
| Q040 | 4010 | ok | ok |
| Q041 | 39 | ok | ok |
| Q042 | 1520 | ok | ok |
| Q043 | 3022 | ok | ok |
| Q044 | 60 | ok | ok |
| Q045 | 1107 | ok | ok |
| Q046 | 3308 | ok | ok |
| Q047 | 1 | ok | ok |
| Q048 | 2377 | ok | ok |
| Q049 | 4702 | ok | ok |
| Q050 | 81 | ok | ok |
| Q051 | 1942 | ok | ok |
| Q052 | 86764 | ok | ok |
| Q053 | 2387 | ok | ok |
| Q054 | 66 | ok | ok |
| Q055 | 112778 | ok | ok |
| Q056 | 11672 | ok | ok |
| Q057 | 4177 | ok | ok |
| Q058 | 0 | ok | ok |
| Q059 | 4177 | ok | ok |
| Q060 | 1187 | ok | ok |
| Q061 | 1 | ok | ok |
| Q062 | 3 | ok | ok |
| Q063 | 1 | ok | ok |
| Q064 | 1 | ok | ok |
| Q065 | 207 | ok | ok |
| Q066 | 1 | ok | ok |
| Q067 | 1 | ok | ok |
| Q068 | 1 | ok | ok |
| Q069 | 4010 | ok | ok |
| Q070 | 1 | ok | ok |
| Q071 | 1003 | ok | ok |
| Q072 | 1287 | ok | ok |
| Q073 | 225 | ok | ok |
| Q074 | 4927 | ok | ok |
| Q075 | 645 | ok | ok |
| Q076 | 2206 | ok | ok |
| Q077 | 818 | ok | ok |
| Q078 | 7632 | ok | ok |
| Q079 | 351 | ok | ok |
| Q080 | 8 | ok | ok |
| Q081 | 888 | ok | ok |
| Q082 | 3306 | ok | ok |
| Q083 | 4040 | ok | ok |
| Q084 | 4922 | ok | ok |
| Q085 | 4925 | ok | ok |
| Q086 | 4903 | ok | ok |
| Q087 | 4903 | ok | ok |
| Q088 | 5 | ok | ok |
| Q089 | 3653 | ok | ok |
| Q090 | 0 | ok | ok |
| Q091 | 3911 | ok | ok |
| Q092 | 0 | ok | ok |
| Q093 | 3856 | ok | ok |
| Q094 | 3864 | ok | ok |
| Q095 | 0 | ok | ok |
| Q096 | 70390 | ok | ok |
| Q097 | 0 | ok | ok |
| Q098 | 0 | ok | ok |
| Q099 | 59586 | ok | ok |
| Q100 | 0 | ok | ok |
| Q101 | 1 | ok | ok |
| Q102 | 0 | ok | ok |
| Q103 | 4108 | ok | ok |
| Q104 | 106 | ok | ok |
| Q105 | 2684 | ok | ok |
| Q106 | 1326 | ok | ok |
| Q107 | 2684 | ok | ok |
| Q108 | 2684 | ok | ok |
| Q109 | 2651 | ok | ok |
| Q110 | 4911 | ok | ok |
| Q111 | 4928 | ok | ok |
| Q112 | 25 | ok | ok |
| Q113 | 951 | ok | ok |
| Q114 | 3977 | ok | ok |
| Q115 | 244 | ok | ok |
| Q116 | 70428 | ok | ok |
| Q117 | 63520 | ok | ok |
| Q118 | 254 | ok | ok |
| Q119 | 133948 | ok | ok |
| Q120 | 42700 | ok | ok |
| Q121 | 341 | ok | ok |
| Q122 | 67 | ok | ok |
| Q123 | 4153 | ok | ok |
| Q124 | 0 | ok | ok |
| Q125 | 8 | ok | ok |
| Q126 | 203 | ok | ok |
| Q127 | 203 | ok | ok |
| Q128 | 3257 | ok | ok |
| Q129 | 3257 | ok | ok |
| Q130 | 3257 | ok | ok |
| Q131 | 3308 | ok | ok |
| Q132 | 3308 | ok | ok |
| Q133 | 81 | ok | ok |
| Q134 | 133948 | ok | ok |
| Q135 | 133948 | ok | ok |
| Q136 | 133948 | ok | ok |
| Q137 | 81 | ok | ok |
| Q138 | 81 | ok | ok |
| Q139 | 81 | ok | ok |
| Q140 | 341 | ok | ok |
| Q141 | 823 | ok | ok |
| Q142 | 11 | ok | ok |
| Q143 | 2057 | ok | ok |
| Q144 | 3939 | ok | ok |
| Q145 | 40947 | ok | ok |
| Q146 | 62 | ok | ok |
| Q147 | 47 | ok | ok |
| Q148 | 4105 | ok | ok |
| Q149 | 95 | ok | ok |
| Q150 | 390 | ok | ok |

Reproduce: `python3 bench/verify_jq_oracle.py --result-dir /tmp/objekte-jq-oracle`.
