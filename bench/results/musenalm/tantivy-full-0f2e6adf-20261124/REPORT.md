# Musenalm release comparison

Revision `0f2e6adfa8e3140f8492c1e835efe97d69942a8c`. Verified 108 of 220 selected predicates and 94 source pages.

Page timings cover locator tuples with evidence and payloads projected away. Ratios above 1 favor Objekte. Build times are one observation per engine and schema mode.

Objekte historical built 148,486 visible records in 3.493 s. Peak RSS was 451.0 MiB and resident RSS at query readiness was 407.8 MiB. It has 15 searchable segments and index byte size unavailable from the in-memory core.
Objekte extended built 148,486 visible records in 3.651 s. Peak RSS was 503.5 MiB and resident RSS at query readiness was 460.6 MiB. It has 15 searchable segments and index byte size unavailable from the in-memory core.
Tantivy historical built 148,486 visible records in 6.487 s. Peak RSS was 402.5 MiB and resident RSS at query readiness was 394.2 MiB. It has 5 searchable segments and 180.3 MiB of managed RAM-directory segment files.
Tantivy extended built 148,486 visible records in 6.435 s. Peak RSS was 393.9 MiB and resident RSS at query readiness was 385.5 MiB. It has 5 searchable segments and 180.3 MiB of managed RAM-directory segment files.

## Per-request samples

### array and nested filters — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q081 places ?= "d7kbujq7399g22j" | entries | typed_json | 888 | 0 / 50 | 7 | 3525.8 [3479.7–3625.5] µs | 517.0 [502.1–536.6] µs | 0.147 |
| Q083 !(places ?= "d7kbujq7399g22j") | entries | typed_json | 4,040 | 0 / 50 | 7 | 3481.5 [3390.0–3536.2] µs | 472.6 [456.8–481.5] µs | 0.136 |
| Q084 language ?= "ger" | entries | typed_json | 4,922 | 0 / 50 | 7 | 17.5 [17.1–21.3] µs | 453.3 [440.8–462.7] µs | 25.9 |
| Q086 carrier_type ?= "Band" | entries | typed_json | 4,903 | 0 / 50 | 7 | 14.5 [14.2–14.9] µs | 451.3 [445.1–457.0] µs | 31.2 |
| Q087 content_type ?= "Text" | entries | typed_json | 4,903 | 0 / 50 | 7 | 14.1 [13.9–15.3] µs | 446.9 [439.3–456.9] µs | 31.8 |
| Q088 agents.agent ?= "lngx6eyfw5qrzas" | entries | typed_json | 5 | 0 / 50 | 7 | 47.5 [45.9–48.1] µs | 67.3 [63.2–69.8] µs | 1.41 |
| Q089 agents.type ?= "Verlag" | entries | typed_json | 3,653 | 0 / 50 | 7 | 21.2 [20.8–22.1] µs | 503.6 [488.0–530.1] µs | 23.8 |
| Q090 agents.uncertain ?= true | entries | typed_json | 0 | 0 / 50 | 7 | 6.6 [6.3–8.5] µs | 15.4 [15.0–16.0] µs | 2.34 |
| Q091 items.owner ?= "Theodor Springmann Stiftung" | entries | typed_json | 3,911 | 0 / 50 | 7 | 21.1 [21.0–21.7] µs | 497.6 [490.9–509.2] µs | 23.6 |
| Q092 items.public ?= true | entries | typed_json | 0 | 0 / 50 | 7 | 6.5 [6.3–6.7] µs | 15.5 [14.8–16.0] µs | 2.39 |
| Q093 items.media ?= "Original" | entries | typed_json | 3,856 | 0 / 50 | 7 | 20.1 [19.9–24.2] µs | 497.3 [485.3–533.2] µs | 24.7 |
| Q094 musenalm_deprecated.status ?= "Original vorha | entries | typed_json | 3,864 | 0 / 50 | 7 | 20.6 [20.5–24.8] µs | 494.2 [488.2–513.1] µs | 23.9 |
| Q095 contributors.agents ?= "lngx6eyfw5qrzas" | contents | typed_json | 0 | 0 / 50 | 7 | 81.8 [81.1–84.0] µs | 16.6 [15.9–17.3] µs | 0.202 |
| Q096 contributors.type ?= "Autor:in" | contents | typed_json | 70,390 | 0 / 50 | 7 | 94.0 [91.5–105.3] µs | 833.9 [829.2–863.6] µs | 8.87 |
| Q097 contributors.conjecture ?= true | contents | typed_json | 0 | 0 / 50 | 7 | 55.2 [54.6–59.2] µs | 15.9 [15.4–16.1] µs | 0.287 |
| Q098 contributors.uncertain ?= true | contents | typed_json | 0 | 0 / 50 | 7 | 54.5 [54.3–57.1] µs | 15.2 [15.1–15.8] µs | 0.28 |
| Q100 language ?= "ger" | contents | typed_json | 0 | 0 / 50 | 7 | 77657.1 [76983.2–78546.8] µs | 16.5 [16.1–17.0] µs | 0.000213 |
| Q101 entries ?= "00fx8ln651na0e3" | series | typed_json | 1 | 0 / 50 | 7 | 23.7 [23.3–25.5] µs | 19.5 [19.1–20.8] µs | 0.824 |
| Q102 related_entries.entry ?= "00fx8ln651na0e3" | series | typed_json | 0 | 0 / 50 | 7 | 18.8 [18.3–19.5] µs | 14.5 [14.2–15.3] µs | 0.774 |
| Q103 media ?= "Original" | items | typed_json | 4,108 | 0 / 50 | 7 | 26.7 [25.2–27.5] µs | 151.9 [143.0–158.6] µs | 5.69 |
| Q104 data.gnd.gender.label ?= "Weiblich" | agents | typed_json | 106 | 0 / 50 | 7 | 68.7 [67.7–69.9] µs | 2022.4 [1986.0–2066.6] µs | 29.4 |
### array and nested filters — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q081 places ?= "d7kbujq7399g22j" | entries | typed_json | 888 | 0 / 0 | 7 | 3764.6 [3643.2–3944.4] µs | 11.1 [10.4–11.1] µs | 0.00294 |
| Q083 !(places ?= "d7kbujq7399g22j") | entries | typed_json | 4,040 | 0 / 0 | 7 | 3547.4 [3406.2–3912.8] µs | 31.9 [31.0–32.6] µs | 0.00898 |
| Q084 language ?= "ger" | entries | typed_json | 4,922 | 0 / 0 | 7 | 14.3 [13.8–20.1] µs | 15.5 [15.1–15.5] µs | 1.08 |
| Q086 carrier_type ?= "Band" | entries | typed_json | 4,903 | 0 / 0 | 7 | 9.6 [9.3–9.9] µs | 14.6 [14.3–14.6] µs | 1.52 |
| Q087 content_type ?= "Text" | entries | typed_json | 4,903 | 0 / 0 | 7 | 8.8 [8.4–9.1] µs | 14.1 [13.8–14.3] µs | 1.61 |
| Q088 agents.agent ?= "lngx6eyfw5qrzas" | entries | typed_json | 5 | 0 / 0 | 7 | 61.6 [57.5–87.6] µs | 5.0 [4.8–5.2] µs | 0.0806 |
| Q089 agents.type ?= "Verlag" | entries | typed_json | 3,653 | 0 / 0 | 7 | 14.7 [14.5–15.3] µs | 16.6 [14.2–16.8] µs | 1.13 |
| Q090 agents.uncertain ?= true | entries | typed_json | 0 | 0 / 0 | 7 | 8.3 [8.2–8.8] µs | 1.4 [1.3–1.4] µs | 0.165 |
| Q091 items.owner ?= "Theodor Springmann Stiftung" | entries | typed_json | 3,911 | 0 / 0 | 7 | 16.0 [15.5–16.2] µs | 15.7 [15.4–15.9] µs | 0.98 |
| Q092 items.public ?= true | entries | typed_json | 0 | 0 / 0 | 7 | 7.4 [7.2–7.7] µs | 1.5 [1.4–1.6] µs | 0.198 |
| Q093 items.media ?= "Original" | entries | typed_json | 3,856 | 0 / 0 | 7 | 14.2 [14.1–14.7] µs | 13.9 [13.8–14.6] µs | 0.979 |
| Q094 musenalm_deprecated.status ?= "Original vorha | entries | typed_json | 3,864 | 0 / 0 | 7 | 15.3 [14.9–15.4] µs | 15.5 [15.2–16.2] µs | 1.02 |
| Q095 contributors.agents ?= "lngx6eyfw5qrzas" | contents | typed_json | 0 | 0 / 0 | 7 | 129.0 [126.5–156.7] µs | 3.2 [3.1–3.3] µs | 0.0247 |
| Q096 contributors.type ?= "Autor:in" | contents | typed_json | 70,390 | 0 / 0 | 7 | 88.9 [86.3–93.7] µs | 106.9 [106.6–128.0] µs | 1.2 |
| Q097 contributors.conjecture ?= true | contents | typed_json | 0 | 0 / 0 | 7 | 61.4 [60.1–70.3] µs | 1.8 [1.6–2.0] µs | 0.0296 |
| Q098 contributors.uncertain ?= true | contents | typed_json | 0 | 0 / 0 | 7 | 59.3 [58.2–61.1] µs | 1.5 [1.4–1.7] µs | 0.0249 |
| Q100 language ?= "ger" | contents | typed_json | 0 | 0 / 0 | 7 | 78376.1 [77900.4–79113.0] µs | 2.8 [2.6–3.0] µs | 3.58e-05 |
| Q101 entries ?= "00fx8ln651na0e3" | series | typed_json | 1 | 0 / 0 | 7 | 47.6 [46.4–50.2] µs | 2.9 [2.7–3.0] µs | 0.061 |
| Q102 related_entries.entry ?= "00fx8ln651na0e3" | series | typed_json | 0 | 0 / 0 | 7 | 20.1 [19.6–20.7] µs | 2.3 [2.2–2.7] µs | 0.116 |
| Q103 media ?= "Original" | items | typed_json | 4,108 | 0 / 0 | 7 | 16.0 [15.7–16.9] µs | 11.2 [11.0–11.3] µs | 0.701 |
| Q104 data.gnd.gender.label ?= "Weiblich" | agents | typed_json | 106 | 0 / 0 | 7 | 34.5 [34.1–36.4] µs | 4.7 [4.6–5.1] µs | 0.135 |
### extended requests — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q151 SORT BY year DESC, preferred_title ASC | entries | none | 4,928 | 0 / 0 | 7 | 2.1 [1.4–2.4] µs | 4.3 [3.0–5.1] µs | 2.06 |
| Q152, Q190 year >= 1800 SORT BY year ASC | entries | typed_json | 3,821 | 0 / 0 | 7 | 152.1 [141.1–171.1] µs | 94.6 [65.6–111.7] µs | 0.622 |
| Q153 Almanach | entries | derived_literal_term_dictionary | 1,316 | 0 / 0 | 7 | 18930.1 [18624.5–19997.6] µs | 273510.5 [271916.2–273995.9] µs | 14.4 |
### literal FTS — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q001 Almanach | * | derived_literal_term_dictionary | 3,269 | 0 / 50 | 7 | 38339.9 [37626.2–39098.9] µs | 261632.4 [260687.1–266711.8] µs | 6.82 |
| Q002 Taschenbuch | * | derived_literal_term_dictionary | 5,608 | 0 / 50 | 7 | 51588.3 [50864.2–53745.5] µs | 264215.0 [260722.3–319403.5] µs | 5.12 |
| Q003 Leipzig | * | derived_literal_term_dictionary | 2,866 | 0 / 50 | 7 | 23133.3 [22686.9–23504.2] µs | 264426.0 [261414.3–265812.3] µs | 11.4 |
| Q004 Wien | * | derived_literal_term_dictionary | 2,995 | 0 / 50 | 7 | 55487.3 [52661.4–56116.8] µs | 263750.8 [261716.2–267209.0] µs | 4.75 |
| Q005 Goethe | * | derived_literal_term_dictionary | 516 | 0 / 50 | 7 | 22235.4 [21106.2–22618.7] µs | 263637.3 [262054.9–266097.6] µs | 11.9 |
| Q006 ü | * | derived_literal_term_dictionary | 65,548 | 0 / 50 | 7 | 799755.2 [788793.2–814920.4] µs | 268921.5 [267816.7–270723.5] µs | 0.336 |
| Q007 a | * | derived_literal_term_dictionary | 147,798 | 0 / 50 | 7 | 796107.8 [788619.8–836278.6] µs | 297026.9 [296624.2–300419.5] µs | 0.373 |
| Q008 __objekte_missing_7c9f31__ | * | derived_literal_term_dictionary | 0 | 0 / 50 | 7 | 41.7 [40.5–46.2] µs | 261279.9 [258437.0–261872.6] µs | 6.26e+03 |
| Q009 Leipzig | places | derived_literal_term_dictionary | 1 | 0 / 50 | 7 | 7.4 [7.0–7.7] µs | 259867.6 [258862.3–318849.8] µs | 3.52e+04 |
| Q010 Wien | places | derived_literal_term_dictionary | 1 | 0 / 50 | 7 | 5.2 [5.1–5.5] µs | 259504.0 [259289.3–261126.7] µs | 5.04e+04 |
| Q011 Goethe | agents | derived_literal_term_dictionary | 62 | 0 / 50 | 7 | 17521.4 [16856.9–17674.3] µs | 262054.5 [260533.7–262516.6] µs | 15 |
| Q012 Schiller | agents | derived_literal_term_dictionary | 31 | 0 / 50 | 7 | 33988.8 [32748.7–34417.5] µs | 261715.7 [260306.9–265665.4] µs | 7.7 |
| Q013 Müller | agents | derived_literal_term_dictionary | 57 | 0 / 50 | 7 | 49861.4 [48703.8–50963.7] µs | 261851.0 [261230.5–263784.0] µs | 5.25 |
| Q014 Text | agents | derived_literal_term_dictionary | 1,742 | 0 / 50 | 7 | 58199.4 [54514.1–58794.3] µs | 262429.3 [261375.1–265647.6] µs | 4.51 |
| Q015 https:// | agents | derived_literal_term_dictionary | 2,684 | 0 / 50 | 7 | 57999.3 [55227.6–59113.3] µs | 265661.9 [263234.0–271514.4] µs | 4.58 |
| Q016 Almanach | series | derived_literal_term_dictionary | 325 | 0 / 50 | 7 | 1090.1 [1064.7–1136.0] µs | 260812.3 [259403.5–261894.2] µs | 239 |
| Q017 Taschenbuch | series | derived_literal_term_dictionary | 390 | 0 / 50 | 7 | 1046.7 [1020.5–1167.3] µs | 260235.2 [259658.1–262937.6] µs | 249 |
| Q018 Vergnügen | entries | derived_literal_term_dictionary | 228 | 0 / 50 | 7 | 3225.6 [3131.9–3308.1] µs | 261834.8 [259750.5–264904.1] µs | 81.2 |
| Q019 Almanach | entries | derived_literal_term_dictionary | 1,316 | 0 / 50 | 7 | 13488.9 [13115.7–13819.0] µs | 259884.1 [259480.5–263904.6] µs | 19.3 |
| Q020 1808 | entries | derived_literal_term_dictionary | 163 | 0 / 50 | 7 | 2436.3 [2358.9–2543.1] µs | 259704.8 [258756.1–315545.9] µs | 107 |
| Q021 WIEN | entries | derived_literal_term_dictionary | 230 | 0 / 50 | 7 | 7312.1 [7057.0–7444.6] µs | 259868.0 [258297.2–264778.2] µs | 35.5 |
| Q022 unbezeichnet | entries | derived_literal_term_dictionary | 1,493 | 0 / 50 | 7 | 11864.3 [11490.5–12322.0] µs | 260257.9 [258388.7–263021.1] µs | 21.9 |
| Q023 Liebe | contents | derived_literal_term_dictionary | 5,307 | 0 / 50 | 7 | 65349.1 [63398.0–68121.0] µs | 260764.8 [259486.4–263028.4] µs | 3.99 |
| Q024 Frühling | contents | derived_literal_term_dictionary | 1,927 | 0 / 50 | 7 | 24541.1 [23425.1–25970.1] µs | 260144.7 [258141.7–262137.4] µs | 10.6 |
| Q025 Lied | contents | derived_literal_term_dictionary | 60,319 | 0 / 50 | 7 | 344147.2 [332240.5–349260.0] µs | 261532.8 [260065.3–266012.2] µs | 0.76 |
| Q026 Der | contents | derived_literal_term_dictionary | 18,825 | 0 / 50 | 7 | 404863.3 [391534.4–419146.7] µs | 265138.8 [260679.5–268185.0] µs | 0.655 |
| Q027 1808 | contents | derived_literal_term_dictionary | 215 | 0 / 50 | 7 | 3295.0 [3123.6–3421.8] µs | 260994.7 [259191.2–265087.2] µs | 79.2 |
| Q028 Theodor Springmann Stiftung | items | derived_literal_term_dictionary | 4,177 | 0 / 50 | 7 | 11086.5 [10811.7–11423.7] µs | 260606.2 [259177.3–266911.1] µs | 23.5 |
| Q029 Musenalm | pages | derived_literal_term_dictionary | 7 | 0 / 50 | 7 | 23.3 [21.7–24.4] µs | 262365.8 [258718.1–265911.3] µs | 1.13e+04 |
| Q030 Taschenbuch | entries | derived_literal_term_dictionary | 1,264 | 0 / 50 | 7 | 9438.3 [9344.9–9573.9] µs | 54094.6 [53672.6–55492.0] µs | 5.73 |
### literal FTS — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q001 Almanach | * | derived_literal_term_dictionary | 3,269 | 0 / 0 | 7 | 39498.2 [38679.4–40252.1] µs | 260428.2 [258776.4–267340.4] µs | 6.59 |
| Q002 Taschenbuch | * | derived_literal_term_dictionary | 5,608 | 0 / 0 | 7 | 53096.3 [51075.8–55132.2] µs | 260534.4 [259430.6–265792.8] µs | 4.91 |
| Q003 Leipzig | * | derived_literal_term_dictionary | 2,866 | 0 / 0 | 7 | 23806.2 [23399.4–24965.9] µs | 261132.4 [259741.2–264854.9] µs | 11 |
| Q004 Wien | * | derived_literal_term_dictionary | 2,995 | 0 / 0 | 7 | 55009.0 [53994.8–56723.5] µs | 260567.8 [258463.9–265685.3] µs | 4.74 |
| Q005 Goethe | * | derived_literal_term_dictionary | 516 | 0 / 0 | 7 | 22912.1 [21381.3–23218.4] µs | 259649.0 [258477.5–262920.2] µs | 11.3 |
| Q006 ü | * | derived_literal_term_dictionary | 65,548 | 0 / 0 | 7 | 790411.2 [771949.5–810249.4] µs | 266984.2 [266159.9–268818.0] µs | 0.338 |
| Q007 a | * | derived_literal_term_dictionary | 147,798 | 0 / 0 | 7 | 795782.4 [785381.3–811484.1] µs | 295291.2 [293942.0–297542.7] µs | 0.371 |
| Q008 __objekte_missing_7c9f31__ | * | derived_literal_term_dictionary | 0 | 0 / 0 | 7 | 140.2 [135.1–141.9] µs | 260229.6 [259248.0–262308.9] µs | 1.86e+03 |
| Q009 Leipzig | places | derived_literal_term_dictionary | 1 | 0 / 0 | 7 | 15.3 [14.4–15.7] µs | 269895.2 [267789.9–272384.5] µs | 1.77e+04 |
| Q010 Wien | places | derived_literal_term_dictionary | 1 | 0 / 0 | 7 | 7.9 [7.4–8.1] µs | 268308.1 [267722.4–269432.3] µs | 3.38e+04 |
| Q011 Goethe | agents | derived_literal_term_dictionary | 62 | 0 / 0 | 7 | 18131.9 [17470.9–18297.3] µs | 268224.4 [267225.4–269422.3] µs | 14.8 |
| Q012 Schiller | agents | derived_literal_term_dictionary | 31 | 0 / 0 | 7 | 33528.5 [30308.3–34000.2] µs | 268629.2 [267278.3–269812.5] µs | 8.01 |
| Q013 Müller | agents | derived_literal_term_dictionary | 57 | 0 / 0 | 7 | 49703.3 [45501.1–50155.8] µs | 268857.7 [267658.8–270194.6] µs | 5.41 |
| Q014 Text | agents | derived_literal_term_dictionary | 1,742 | 0 / 0 | 7 | 58936.2 [54206.1–61055.0] µs | 269346.4 [268548.4–271672.6] µs | 4.57 |
| Q015 https:// | agents | derived_literal_term_dictionary | 2,684 | 0 / 0 | 7 | 59563.1 [54062.9–60400.9] µs | 273058.6 [271714.8–278949.7] µs | 4.58 |
| Q016 Almanach | series | derived_literal_term_dictionary | 325 | 0 / 0 | 7 | 1386.2 [1364.5–1416.8] µs | 270748.0 [268880.0–274538.3] µs | 195 |
| Q017 Taschenbuch | series | derived_literal_term_dictionary | 390 | 0 / 0 | 7 | 1269.9 [1260.2–1319.3] µs | 269580.2 [267887.5–272656.6] µs | 212 |
| Q018 Vergnügen | entries | derived_literal_term_dictionary | 228 | 0 / 0 | 7 | 3429.4 [3276.2–3655.2] µs | 269007.8 [267668.4–274055.5] µs | 78.4 |
| Q019 Almanach | entries | derived_literal_term_dictionary | 1,316 | 0 / 0 | 7 | 13795.4 [13009.1–14290.1] µs | 270087.5 [268566.8–272760.3] µs | 19.6 |
| Q020 1808 | entries | derived_literal_term_dictionary | 163 | 0 / 0 | 7 | 2567.5 [2415.7–2627.1] µs | 268532.2 [267604.8–270013.3] µs | 105 |
| Q021 WIEN | entries | derived_literal_term_dictionary | 230 | 0 / 0 | 7 | 7421.1 [7093.4–7626.9] µs | 268934.3 [268366.6–271234.6] µs | 36.2 |
| Q022 unbezeichnet | entries | derived_literal_term_dictionary | 1,493 | 0 / 0 | 7 | 12373.1 [11565.9–12738.3] µs | 269210.1 [267349.1–270902.1] µs | 21.8 |
| Q023 Liebe | contents | derived_literal_term_dictionary | 5,307 | 0 / 0 | 7 | 66048.1 [65173.2–68363.7] µs | 269340.5 [268155.1–270969.0] µs | 4.08 |
| Q024 Frühling | contents | derived_literal_term_dictionary | 1,927 | 0 / 0 | 7 | 24982.5 [24335.8–25521.0] µs | 269028.4 [267343.4–269897.6] µs | 10.8 |
| Q025 Lied | contents | derived_literal_term_dictionary | 60,319 | 0 / 0 | 7 | 346930.1 [341484.6–394943.1] µs | 269907.2 [268363.8–272166.6] µs | 0.778 |
| Q026 Der | contents | derived_literal_term_dictionary | 18,825 | 0 / 0 | 7 | 410125.8 [401116.9–422673.9] µs | 271197.6 [269601.9–274929.4] µs | 0.661 |
| Q027 1808 | contents | derived_literal_term_dictionary | 215 | 0 / 0 | 7 | 3482.2 [3353.7–3657.5] µs | 269772.0 [267227.7–271721.6] µs | 77.5 |
| Q028 Theodor Springmann Stiftung | items | derived_literal_term_dictionary | 4,177 | 0 / 0 | 7 | 11603.1 [11359.6–11815.9] µs | 270161.9 [268155.7–273300.9] µs | 23.3 |
| Q029 Musenalm | pages | derived_literal_term_dictionary | 7 | 0 / 0 | 7 | 36.7 [33.7–37.5] µs | 269716.8 [267402.3–274128.3] µs | 7.35e+03 |
| Q030 Taschenbuch | entries | derived_literal_term_dictionary | 1,264 | 0 / 0 | 7 | 9836.6 [9741.4–9929.4] µs | 53347.0 [52479.4–54910.5] µs | 5.42 |
### mixed requests — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q142 Wien | entries | derived_literal_and_typed_json | 11 | 0 / 50 | 7 | 155.5 [149.8–164.6] µs | 260915.8 [259348.5–266527.7] µs | 1.68e+03 |
| Q144 Liebe | contents | derived_literal_and_typed_json | 3,939 | 0 / 50 | 7 | 63877.0 [62820.0–66202.2] µs | 261807.5 [260303.9–266954.5] µs | 4.1 |
| Q145 Lied | contents | derived_literal_and_typed_json | 40,947 | 0 / 50 | 7 | 319357.1 [310817.9–348803.2] µs | 243469.3 [242007.4–248119.9] µs | 0.762 |
| Q146 Goethe | agents | derived_literal_and_typed_json | 62 | 0 / 50 | 7 | 32228.1 [31870.0–32353.9] µs | 46382.2 [46000.2–46593.9] µs | 1.44 |
| Q148 Original | items | derived_literal_and_typed_json | 4,105 | 0 / 50 | 7 | 14280.0 [13954.8–14376.9] µs | 19109.1 [18980.9–20191.2] µs | 1.34 |
### mixed requests — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q141 Almanach | entries | derived_literal_and_typed_json | 823 | 0 / 0 | 7 | 12714.8 [12399.4–12925.6] µs | 269555.6 [268782.1–276851.7] µs | 21.2 |
| Q142 Wien | entries | derived_literal_and_typed_json | 11 | 0 / 0 | 7 | 190.3 [181.0–195.5] µs | 270212.0 [269762.3–277050.1] µs | 1.42e+03 |
| Q143 Taschenbuch | entries | derived_literal_and_typed_json | 2,057 | 0 / 0 | 7 | 28241.3 [27510.7–28669.2] µs | 87958.0 [87462.8–92531.6] µs | 3.11 |
| Q144 Liebe | contents | derived_literal_and_typed_json | 3,939 | 0 / 0 | 7 | 64512.4 [62998.1–65577.3] µs | 274177.2 [269768.3–276038.4] µs | 4.25 |
| Q145 Lied | contents | derived_literal_and_typed_json | 40,947 | 0 / 0 | 7 | 323389.8 [312800.8–329044.7] µs | 250495.4 [248919.0–254170.1] µs | 0.775 |
| Q146 Goethe | agents | derived_literal_and_typed_json | 62 | 0 / 0 | 7 | 32342.5 [31996.8–33065.9] µs | 45160.2 [44927.8–45348.4] µs | 1.4 |
| Q148 Original | items | derived_literal_and_typed_json | 4,105 | 0 / 0 | 7 | 15053.3 [14767.3–15777.0] µs | 19808.4 [19613.7–20983.7] µs | 1.32 |
### scalar and numeric filters — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q031 fictional = false | places | typed_json | 203 | 0 / 50 | 7 | 18.0 [17.0–43.4] µs | 93.1 [92.4–100.1] µs | 5.16 |
| Q032 fictional = true | places | typed_json | 7 | 0 / 50 | 7 | 7.9 [7.7–8.1] µs | 27.9 [27.5–30.0] µs | 3.54 |
| Q033 edit_state = "Edited" | places | typed_json | 210 | 0 / 50 | 7 | 11.9 [11.5–12.4] µs | 85.6 [84.3–88.2] µs | 7.19 |
| Q034 @record.id = "d7kbujq7399g22j" | places | typed_json | 1 | 0 / 50 | 7 | 4.0 [3.8–4.5] µs | 18.1 [17.5–20.9] µs | 4.49 |
| Q035 musenalm_id > 100 | places | typed_json | 124 | 0 / 50 | 7 | 20.5 [20.2–22.3] µs | 103.7 [100.7–110.7] µs | 5.05 |
| Q036 musenalm_id <= 100 | places | typed_json | 86 | 0 / 50 | 7 | 18.1 [17.7–18.8] µs | 103.5 [99.8–109.2] µs | 5.71 |
| Q037 corporate_body = true | agents | typed_json | 753 | 0 / 50 | 7 | 57.3 [56.5–63.8] µs | 284.0 [272.7–292.6] µs | 4.95 |
| Q038 corporate_body = false | agents | typed_json | 3,257 | 0 / 50 | 7 | 45.0 [44.0–47.5] µs | 1507.3 [1499.5–1545.4] µs | 33.5 |
| Q039 fictional = true | agents | typed_json | 0 | 0 / 50 | 7 | 22.4 [22.0–22.6] µs | 20.8 [20.3–22.3] µs | 0.929 |
| Q040 fictional = false | agents | typed_json | 4,010 | 0 / 50 | 7 | 32.1 [31.9–32.5] µs | 1119.5 [1092.7–1141.0] µs | 34.9 |
| Q041 edit_state = "Review" | agents | typed_json | 39 | 0 / 50 | 7 | 65.3 [64.1–65.6] µs | 1400.7 [1370.3–1471.2] µs | 21.5 |
| Q042 profession = "Text" | agents | typed_json | 1,520 | 0 / 50 | 7 | 45.6 [45.2–47.6] µs | 1749.4 [1716.7–1784.6] µs | 38.3 |
| Q043 musenalm_id >= 1000 | agents | typed_json | 3,022 | 0 / 50 | 7 | 1869.5 [1808.5–1921.8] µs | 1193.7 [1164.8–1215.6] µs | 0.639 |
| Q044 year = 1808 | entries | typed_json | 60 | 0 / 50 | 7 | 41.2 [40.7–46.5] µs | 470.3 [440.7–480.9] µs | 11.4 |
| Q045 year < 1800 | entries | typed_json | 1,107 | 0 / 50 | 7 | 51.4 [50.6–68.0] µs | 557.8 [534.9–567.7] µs | 10.9 |
| Q046 year >= 1800 && year < 1850 | entries | typed_json | 3,308 | 0 / 50 | 7 | 311.0 [310.5–314.5] µs | 672.7 [646.6–685.1] µs | 2.16 |
| Q047 @record.id = "00fx8ln651na0e3" | entries | typed_json | 1 | 0 / 50 | 7 | 13.5 [12.4–13.7] µs | 26.5 [25.6–28.2] µs | 1.97 |
| Q048 edit_state = "Edited" | entries | typed_json | 2,377 | 0 / 50 | 7 | 20.7 [20.2–21.1] µs | 519.9 [483.9–533.5] µs | 25.1 |
| Q049 pseudonym = false | entries | typed_json | 4,702 | 0 / 50 | 7 | 14.7 [14.2–14.9] µs | 451.3 [429.6–454.1] µs | 30.7 |
| Q050 entry = "00fx8ln651na0e3" | contents | typed_json | 81 | 0 / 50 | 7 | 298.6 [296.4–327.7] µs | 298.0 [286.1–350.0] µs | 0.998 |
| Q051 year = 1808 | contents | typed_json | 1,942 | 0 / 50 | 7 | 126.6 [124.1–127.3] µs | 417.0 [413.6–448.7] µs | 3.29 |
| Q052 year >= 1800 && year < 1850 | contents | typed_json | 86,764 | 0 / 50 | 7 | 1897.6 [1885.9–1925.8] µs | 2113.7 [2086.1–2121.1] µs | 1.11 |
| Q053 numbering = 1 | contents | typed_json | 2,387 | 0 / 50 | 7 | 116.6 [115.2–119.2] µs | 348.4 [347.9–357.3] µs | 2.99 |
| Q054 numbering > 1 && numbering < 2 | contents | typed_json | 66 | 0 / 50 | 7 | 1760.1 [1747.0–1776.5] µs | 1716.5 [1697.0–1743.8] µs | 0.975 |
| Q055 numbering >= 10 | contents | typed_json | 112,778 | 0 / 50 | 7 | 1497.3 [1481.5–1521.4] µs | 1633.9 [1618.4–1648.7] µs | 1.09 |
| Q056 pseudonym = true | contents | typed_json | 11,672 | 0 / 50 | 7 | 106.0 [104.3–108.0] µs | 391.0 [384.3–398.3] µs | 3.69 |
| Q057 basis = true | items | typed_json | 4,177 | 0 / 50 | 7 | 19.6 [18.5–20.8] µs | 148.7 [146.1–155.3] µs | 7.58 |
| Q058 public = true | items | typed_json | 0 | 0 / 50 | 7 | 6.2 [6.1–6.5] µs | 15.8 [15.5–16.3] µs | 2.54 |
| Q059 owner = "Theodor Springmann Stiftung" | items | typed_json | 4,177 | 0 / 50 | 7 | 15.4 [15.0–16.4] µs | 144.1 [139.8–146.8] µs | 9.35 |
| Q060 frequency = "jährlich" | series | typed_json | 1,187 | 0 / 50 | 7 | 26.2 [25.3–27.3] µs | 130.2 [126.0–138.4] µs | 4.98 |
| Q061 name = "Leipzig" | places | typed_json | 1 | 0 / 50 | 7 | 16.3 [15.9–17.2] µs | 18.5 [17.8–18.7] µs | 1.13 |
### scalar and numeric filters — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q031, Q126, Q127 fictional = false | places | typed_json | 203 | 0 / 0 | 7 | 27.5 [26.7–32.8] µs | 10.1 [9.7–11.0] µs | 0.367 |
| Q032 fictional = true | places | typed_json | 7 | 0 / 0 | 7 | 5.3 [4.8–5.5] µs | 2.5 [2.5–2.6] µs | 0.485 |
| Q033 edit_state = "Edited" | places | typed_json | 210 | 0 / 0 | 7 | 7.4 [7.3–7.5] µs | 8.2 [8.2–8.6] µs | 1.11 |
| Q034 @record.id = "d7kbujq7399g22j" | places | typed_json | 1 | 0 / 0 | 7 | 7.2 [6.8–18.7] µs | 5.3 [4.8–5.7] µs | 0.747 |
| Q035 musenalm_id > 100 | places | typed_json | 124 | 0 / 0 | 7 | 26.5 [26.2–31.4] µs | 30.9 [30.0–31.8] µs | 1.17 |
| Q036 musenalm_id <= 100 | places | typed_json | 86 | 0 / 0 | 7 | 11.9 [11.3–12.4] µs | 22.2 [21.5–24.1] µs | 1.86 |
| Q037 corporate_body = true | agents | typed_json | 753 | 0 / 0 | 7 | 34.1 [33.8–37.1] µs | 9.5 [9.1–10.2] µs | 0.279 |
| Q038, Q128, Q129, Q130 corporate_body = false | agents | typed_json | 3,257 | 0 / 0 | 7 | 31.3 [31.2–31.9] µs | 11.2 [10.6–12.1] µs | 0.358 |
| Q039 fictional = true | agents | typed_json | 0 | 0 / 0 | 7 | 24.1 [23.5–25.1] µs | 2.4 [2.3–2.6] µs | 0.102 |
| Q040 fictional = false | agents | typed_json | 4,010 | 0 / 0 | 7 | 25.4 [25.2–27.0] µs | 11.7 [11.3–11.8] µs | 0.46 |
| Q041 edit_state = "Review" | agents | typed_json | 39 | 0 / 0 | 7 | 27.7 [27.4–29.9] µs | 5.8 [5.4–6.5] µs | 0.208 |
| Q042 profession = "Text" | agents | typed_json | 1,520 | 0 / 0 | 7 | 37.4 [36.8–62.1] µs | 11.0 [10.8–20.6] µs | 0.295 |
| Q043 musenalm_id >= 1000 | agents | typed_json | 3,022 | 0 / 0 | 7 | 2204.7 [2194.1–2269.1] µs | 46.6 [45.7–47.0] µs | 0.0211 |
| Q044 year = 1808 | entries | typed_json | 60 | 0 / 0 | 7 | 13.2 [12.8–26.5] µs | 10.5 [9.9–10.7] µs | 0.793 |
| Q045 year < 1800 | entries | typed_json | 1,107 | 0 / 0 | 7 | 39.4 [38.8–50.7] µs | 58.7 [58.1–59.7] µs | 1.49 |
| Q046, Q131, Q132 year >= 1800 && year < 1850 | entries | typed_json | 3,308 | 0 / 0 | 7 | 324.6 [314.4–354.5] µs | 180.0 [177.9–181.8] µs | 0.555 |
| Q047 @record.id = "00fx8ln651na0e3" | entries | typed_json | 1 | 0 / 0 | 7 | 7.2 [6.8–7.6] µs | 4.6 [4.4–4.7] µs | 0.634 |
| Q048 edit_state = "Edited" | entries | typed_json | 2,377 | 0 / 0 | 7 | 12.6 [12.4–15.2] µs | 16.9 [16.7–17.0] µs | 1.34 |
| Q049 pseudonym = false | entries | typed_json | 4,702 | 0 / 0 | 7 | 9.2 [9.0–9.6] µs | 17.8 [17.4–18.0] µs | 1.93 |
| Q050, Q133, Q137, Q138, Q139 entry = "00fx8ln651na0e3" | contents | typed_json | 81 | 0 / 0 | 7 | 122.5 [119.4–124.8] µs | 8.7 [8.3–9.1] µs | 0.0706 |
| Q051 year = 1808 | contents | typed_json | 1,942 | 0 / 0 | 7 | 79.6 [78.5–105.2] µs | 15.2 [14.6–15.9] µs | 0.191 |
| Q052 year >= 1800 && year < 1850 | contents | typed_json | 86,764 | 0 / 0 | 7 | 2039.8 [2030.4–2161.0] µs | 1011.0 [997.2–1029.7] µs | 0.496 |
| Q053 numbering = 1 | contents | typed_json | 2,387 | 0 / 0 | 7 | 86.1 [82.5–87.6] µs | 20.0 [19.5–20.4] µs | 0.232 |
| Q054 numbering > 1 && numbering < 2 | contents | typed_json | 66 | 0 / 0 | 7 | 1619.3 [1586.4–1675.4] µs | 1036.2 [1033.9–1053.1] µs | 0.64 |
| Q055 numbering >= 10 | contents | typed_json | 112,778 | 0 / 0 | 7 | 1512.5 [1499.8–1582.2] µs | 762.7 [754.0–785.9] µs | 0.504 |
| Q056 pseudonym = true | contents | typed_json | 11,672 | 0 / 0 | 7 | 86.7 [85.0–100.1] µs | 67.5 [66.6–70.9] µs | 0.779 |
| Q057 basis = true | items | typed_json | 4,177 | 0 / 0 | 7 | 10.6 [10.2–35.9] µs | 11.9 [11.6–13.8] µs | 1.12 |
| Q058 public = true | items | typed_json | 0 | 0 / 0 | 7 | 7.6 [7.4–8.0] µs | 1.2 [1.1–1.3] µs | 0.159 |
| Q059 owner = "Theodor Springmann Stiftung" | items | typed_json | 4,177 | 0 / 0 | 7 | 11.3 [11.1–12.0] µs | 11.4 [11.4–12.3] µs | 1.01 |
| Q060 frequency = "jährlich" | series | typed_json | 1,187 | 0 / 0 | 7 | 16.6 [16.1–17.1] µs | 5.1 [5.0–5.4] µs | 0.307 |
| Q061 name = "Leipzig" | places | typed_json | 1 | 0 / 0 | 7 | 34.0 [32.5–39.1] µs | 3.3 [3.1–3.4] µs | 0.0957 |
### sorted pages — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q134 numbering >= 0 | contents | typed_json | 133,948 | 0 / 100 | 7 | 375.9 [371.6–384.9] µs | 2203.6 [2188.8–2286.8] µs | 5.86 |
| Q135 numbering >= 0 | contents | typed_json | 133,948 | 1000 / 100 | 7 | 903.7 [847.6–927.4] µs | 3013.8 [2980.0–3049.7] µs | 3.34 |
| Q136 numbering >= 0 | contents | typed_json | 133,948 | 10000 / 100 | 7 | 5694.8 [5293.3–5969.2] µs | 5257.9 [5213.1–5338.3] µs | 0.923 |
| Q137, Q138, Q139 entry = "00fx8ln651na0e3" | contents | typed_json | 81 | 0 / 50 | 7 | 355.0 [352.0–356.5] µs | 354.9 [349.7–368.1] µs | 1 |
### sorted pages — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q134, Q135, Q136 numbering >= 0 | contents | typed_json | 133,948 | 0 / 0 | 7 | 276.9 [272.8–281.2] µs | 524.6 [520.1–548.6] µs | 1.89 |
### structure and metadata — ordered_locator_page

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q120 incipit_statement = "" | contents | typed_json | 42,700 | 0 / 50 | 7 | 102.6 [93.4–109.4] µs | 594.3 [583.6–596.5] µs | 5.79 |
### structure and metadata — predicate_count

| Request | Collection | Representation | Matches | Offset / page | Samples | Objekte median [min–max] | Tantivy median [min–max] | Tantivy / Objekte |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Q120 incipit_statement = "" | contents | typed_json | 42,700 | 0 / 0 | 7 | 94.9 [92.7–98.9] µs | 91.2 [89.2–94.7] µs | 0.96 |

## Largest absolute median differences

- Q006 ordered_locator_page matched 65,548 records. Objekte 799.755 ms, Tantivy 268.921 ms.
- Q006 predicate_count matched 65,548 records. Objekte 790.411 ms, Tantivy 266.984 ms.
- Q007 predicate_count matched 147,798 records. Objekte 795.782 ms, Tantivy 295.291 ms.
- Q007 ordered_locator_page matched 147,798 records. Objekte 796.108 ms, Tantivy 297.027 ms.
- Q142 predicate_count matched 11 records. Objekte 0.190 ms, Tantivy 270.212 ms.
- Q009 predicate_count matched 1 records. Objekte 0.015 ms, Tantivy 269.895 ms.
- Q029 predicate_count matched 7 records. Objekte 0.037 ms, Tantivy 269.717 ms.
- Q016 predicate_count matched 325 records. Objekte 1.386 ms, Tantivy 270.748 ms.
- Q017 predicate_count matched 390 records. Objekte 1.270 ms, Tantivy 269.580 ms.
- Q010 predicate_count matched 1 records. Objekte 0.008 ms, Tantivy 268.308 ms.

## Unsupported predicates (112)

- Q062 — operator requires another mapping
- Q063 — field modifiers and positional paths require another mapping
- Q064 — operator requires another mapping
- Q065 — operator requires another mapping
- Q066 — operator requires another mapping
- Q067 — field modifiers and positional paths require another mapping
- Q068 — operator requires another mapping
- Q069 — operator requires another mapping
- Q070 — field modifiers and positional paths require another mapping
- Q071 — operator requires another mapping
- Q072 — field modifiers and positional paths require another mapping
- Q073 — operator requires another mapping
- Q074 — operator requires another mapping
- Q075 — operator requires another mapping
- Q076 — operator requires another mapping
- Q077 — field modifiers and positional paths require another mapping
- Q078 — operator requires another mapping
- Q079 — operator requires another mapping
- Q080 — field modifiers and positional paths require another mapping
- Q082 — operator requires another mapping
- Q085 — field modifiers and positional paths require another mapping
- Q099 — operator requires another mapping
- Q105 — operator requires another mapping
- Q106 — field modifiers and positional paths require another mapping
- Q107 — field modifiers and positional paths require another mapping
- Q108 — field modifiers and positional paths require another mapping
- Q109 — metadata path @file.size requires another mapping
- Q110 — field modifiers and positional paths require another mapping
- Q111 — metadata path @record.byte_start requires another mapping
- Q112 — field modifiers and positional paths require another mapping
- Q113 — field modifiers and positional paths require another mapping
- Q114 — field modifiers and positional paths require another mapping
- Q115 — field modifiers and positional paths require another mapping
- Q116 — field modifiers and positional paths require another mapping
- Q117 — field modifiers and positional paths require another mapping
- Q118 — field modifiers and positional paths require another mapping
- Q119 — metadata path @file.modified requires another mapping
- Q121 — field modifiers and positional paths require another mapping
- Q122 — field modifiers and positional paths require another mapping
- Q123 — field modifiers and positional paths require another mapping
- Q124 — field modifiers and positional paths require another mapping
- Q125 — metadata path @collection.name requires another mapping
- Q140 — field modifiers and positional paths require another mapping
- Q147 — field modifiers and positional paths require another mapping
- Q149 — field modifiers and positional paths require another mapping
- Q150 — field modifiers and positional paths require another mapping
- Q154 — metadata path @file.size requires another mapping
- Q155 — field, parameter and clock operands require another mapping
- Q156 — field, parameter and clock operands require another mapping
- Q157 — field, parameter and clock operands require another mapping
- Q158 — field, parameter and clock operands require another mapping
- Q159 — field modifiers and positional paths require another mapping
- Q160 — field modifiers and positional paths require another mapping
- Q161 — metadata path @file.size requires another mapping
- Q162 — metadata path @file.modified requires another mapping
- Q163 — FTS mode requires another mapping
- Q164 — FTS mode requires another mapping
- Q165 — FTS mode requires another mapping
- Q166 — contents.entry.preferred_title is not a declared local source path
- Q167 — contents.entry.year is not a declared local source path
- Q168 — series.entries.preferred_title is not a declared local source path
- Q169 — contents.entry.places.name is not a declared local source path
- Q170 — metadata path @backlinks.contents__entry.preferred_title requires another mapping
- Q171 — entries.agents.agent.name is not a declared local source path
- Q172 — contents.contributors.agents.name is not a declared local source path
- Q173 — FTS mode requires another mapping
- Q174 — contents.entry.year is not a declared local source path
- Q175 — contents.entry.year is not a declared local source path
- Q176 — field, parameter and clock operands require another mapping
- Q177 — contents.entry.year is not a declared local source path
- Q178 — field, parameter and clock operands require another mapping
- Q179 — entries.places.name is not a declared local source path
- Q180 — series.related_entries.entry.preferred_title is not a declared local source path
- Q181 — bound or positional FTS field requires another mapping
- Q182 — FTS mode requires another mapping
- Q183 — metadata path @file.size requires another mapping
- Q184 — metadata path @file.modified requires another mapping
- Q185 — metadata path @file.modified requires another mapping
- Q186 — FTS mode requires another mapping
- Q187 — field modifiers and positional paths require another mapping
- Q188 — FTS mode requires another mapping
- Q189 — operator requires another mapping
- Q191 — contents.entry.year is not a declared local source path
- Q192 — field modifiers and positional paths require another mapping
- Q193 — series.related_entries.entry.preferred_title is not a declared local source path
- Q194 — operator requires another mapping
- Q195 — field modifiers and positional paths require another mapping
- Q196 — field modifiers and positional paths require another mapping
- Q197 — field modifiers and positional paths require another mapping
- Q198 — field modifiers and positional paths require another mapping
- Q199 — field modifiers and positional paths require another mapping
- Q200 — field modifiers and positional paths require another mapping
- Q201 — field modifiers and positional paths require another mapping
- Q202 — field modifiers and positional paths require another mapping
- Q203 — field modifiers and positional paths require another mapping
- Q204 — field modifiers and positional paths require another mapping
- Q205 — field modifiers and positional paths require another mapping
- Q206 — field modifiers and positional paths require another mapping
- Q207 — field modifiers and positional paths require another mapping
- Q208 — field modifiers and positional paths require another mapping
- Q209 — field modifiers and positional paths require another mapping
- Q210 — field modifiers and positional paths require another mapping
- Q211 — field modifiers and positional paths require another mapping
- Q212 — field modifiers and positional paths require another mapping
- Q213 — field modifiers and positional paths require another mapping
- Q214 — field modifiers and positional paths require another mapping
- Q215 — field modifiers and positional paths require another mapping
- Q216 — field modifiers and positional paths require another mapping
- Q217 — field modifiers and positional paths require another mapping
- Q218 — field modifiers and positional paths require another mapping
- Q219 — field modifiers and positional paths require another mapping
- Q220 — field modifiers and positional paths require another mapping

## Verified counts with unsupported pages (14)

- Q126 — sort [SortField { field: "name", descending: false, expression: None }] requires another mapping
- Q127 — sort [SortField { field: "@file.size", descending: true, expression: None }, SortField { field: "name", descending: false, expression: None }] requires another mapping
- Q128 — sort [SortField { field: "name", descending: false, expression: None }] requires another mapping
- Q129 — sort [SortField { field: "name", descending: false, expression: None }] requires another mapping
- Q130 — sort [SortField { field: "name", descending: false, expression: None }] requires another mapping
- Q131 — sort [SortField { field: "year", descending: false, expression: None }, SortField { field: "preferred_title", descending: false, expression: None }] requires another mapping
- Q132 — sort [SortField { field: "year", descending: true, expression: None }, SortField { field: "preferred_title", descending: false, expression: None }] requires another mapping
- Q133 — sort [SortField { field: "numbering", descending: false, expression: None }, SortField { field: "preferred_title", descending: false, expression: None }] requires another mapping
- Q141 — sort [SortField { field: "year", descending: false, expression: None }] requires another mapping
- Q143 — sort [SortField { field: "preferred_title", descending: false, expression: None }] requires another mapping
- Q151 — sort [SortField { field: "year", descending: true, expression: None }, SortField { field: "preferred_title", descending: false, expression: None }] requires another mapping
- Q152 — sort [SortField { field: "year", descending: false, expression: None }] requires another mapping
- Q153 — sort [SortField { field: "preferred_title", descending: true, expression: None }] requires another mapping
- Q190 — sort [SortField { field: "Min(language)", descending: false, expression: Some(SortExpression { path: "language", binding: None, reducer: Some(Min) }) }] requires another mapping

Raw samples, complete inventory, verification counts and run manifest are in this result directory.
