# Decoded-posting clock checkpoint

The baseline binary is the scanner candidate before the clock change, SHA-256 `c8ce6d323e9b3b5de8f6c59eb0a140b96263b9ca2a7cdb8e2ecfa688b2eaa6e6`. The candidate adds the clock read-hit change, SHA-256 `e5a1258dd8dbf6405a4d323b27267964ff9ad93b836246bb13bce4e11fe8f17f`. Both used the same Musenalm fixture, `--extended --incremental --profiles ascii-off --cases Q167,Q175,Q177 --repetitions 30 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32` with memory storage.

| Case | Before median ms | Clock median ms | Ratio |
| --- | ---: | ---: | ---: |
| Q167 | 3.146 | 3.108 | 0.988 |
| Q175 | 2.849 | 2.850 | 1.001 |
| Q177 | 2.511 | 2.605 | 1.038 |

Result digests match. These are sequential fresh-process runs, so the small ratios are noise-sensitive and do not prove a speedup or regression. The full interleaved campaign is the acceptance evidence. Cache capacity and eligible-domain keys are unchanged; the focused cache and workspace tests passed.
