# Query scanner projection and source-read comparison

Revision `5887c7be02` gives source readers a query-mode field projection. The JSON readers deserialize only top-level members needed by the bound filter and selected FTS fields; default raw FTS still reads all fields. Records that may enter a logical page are parsed in full once. A range read checks the opened file's revision once before reading, and the single-record path avoids a second byte copy. Scan matches use one inline slot, and predicates skip matched-field evidence work when the request and result cache do not need it.

The [independent jq oracle](../jq-oracle-5887c7be02/REPORT.md) passed all 300 complete membership comparisons. The [index-free acceptance run](../acceptance-5887c7be02/REPORT.md) passed 150 cases under each alphabet, including ordered pages, payloads and evidence. The [quick gate](../quick-5887c7be02/gate.json) passed 150 samples at 0.799× the prior optimized baseline; the [full gate](../full-5887c7be02/gate.json) passed 3,000 samples at 0.751×. All 36 core tests passed. The [build gate](../build-5887c7be02/REPORT.md) passed all four configurations with three fresh processes each.

The [isolated deprecated-core comparison](../isolated-5887c7be02/REPORT.md) used the same seven schemas and 89 indexed paths in both cores. All 34 counts and ordered pages matched. The earlier new-core times below come from the [previous comparison](../isolated-128a7601c2/REPORT.md); each comparison uses ten samples per case and isolated processes.

| Case | Previous new ms | Current new ms | Current deprecated ms | Current new / deprecated |
|---|---:|---:|---:|---:|
| Q117 | 158.676 | 86.177 | 84.783 | 1.02× |
| Q120 | 167.694 | 81.406 | 82.095 | 0.99× |

The previous new core was roughly twice as slow as the deprecated core on these two full scans. The current core is within 2% on Q117 and slightly faster on Q120.
