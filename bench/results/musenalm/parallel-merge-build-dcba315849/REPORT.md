# Musenalm index-build check after parallel index merging

Measured code: `dcba315849a4b845bf6e207c73934d2bff0731b4` (core merge commit `6ac1de4f2834e1d368e8dd545dba071d65d9e2bf`). Comparison: the recorded pre-merge parallel builder at `d15b689d1ac21709195b79fc382c045d8dec9867`. Both used the same 148,486-record Musenalm corpus, seven schemas, canonical registrations, Rust 1.99.1 release build with default flags, a 13th Gen Intel Core i9-13900K, `performance` governor, CPUs 0–31 affinity, warm filesystem caches, and three fresh processes per configuration. Each build parsed all records, restored no snapshot, and had zero parse failures. Schema digest `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9` and source fingerprint `eec50dbfbfd023aad31e9c3609f4a7f14b177ecc1a022c182321ae704c188cf6` matched. The runner's compatibility gates had no failures.

## Build wall time

Medians and current min–max are milliseconds. The automatic policy resolved to 32 build workers. A one-worker disabled-snapshot control isolates the serial ownership changes from parallel merge scheduling.

| Alphabet / mode | Workers | Before ms | Current ms | Change ms | Change % | Current min–max ms | Peak RSS before→current MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ASCII, no snapshot | 32 | 1,689.7 | 1,896.3 | +206.6 | +12.2% | 1,894.0–1,905.4 | 303.0→327.3 |
| Unicode, no snapshot | 32 | 1,792.7 | 2,102.4 | +309.6 | +17.3% | 2,097.3–2,184.6 | 304.3→333.9 |
| ASCII, snapshot rebuild | 32 | 1,977.0 | 2,260.8 | +283.8 | +14.4% | 2,183.3–2,268.6 | 386.8→414.1 |
| Unicode, snapshot rebuild | 32 | 2,178.0 | 2,404.7 | +226.8 | +10.4% | 2,390.4–2,410.7 | 396.8→421.8 |
| ASCII, no snapshot | 1 | 5,944.7 | 5,999.9 | +55.3 | +0.9% | 5,995.0–6,019.6 | 276.0→278.4 |
| Unicode, no snapshot | 1 | 6,775.5 | 6,823.0 | +47.5 | +0.7% | 6,770.7–7,079.4 | 281.3→279.6 |

**The new parallel merge did not speed up Musenalm construction in this comparison.** The one-worker control is near its recorded baseline, while the automatic path is consistently slower. For disabled snapshots, the extraction/index interval increased by 194 ms ASCII and 309 ms Unicode; discovery rose only 12 and 6 ms. Automatic process CPU time rose from 6.90 to 8.38 seconds ASCII and from 7.70 to 9.39 seconds Unicode. This points to extra work or contention in the concurrent construction path. The present measurements do not isolate the cost of each merge destination or establish allocation as the cause.

The saved semantic index signatures match the pre-merge builder for both alphabets and both snapshot modes; snapshot sizes remain 111,214,033 bytes ASCII and 113,724,609 bytes Unicode. The compatibility gate passing means the regression stayed below its broad failure threshold, not that the change improved speed. These are separate measurement sessions rather than paired old/new process blocks, so small differences can reflect run conditions. The auto-mode changes are substantially larger than the one-worker change and the within-run ranges.

Raw samples, commands, environment, schema artifact and gate results are under [auto](auto/) and [one-disabled](one-disabled/). The earlier [pre-merge build results](../parallel-index-construction-d15b689d1a/) are retained unchanged. No query benchmark or query correctness run was made in this narrowed measurement request.
