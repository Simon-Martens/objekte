# Parallel index construction baseline and verification

Starting branch: `development/local-file-search-core` at `156fe5bc440791dd361d82e7f0f7935c50820f46`. Work proceeds on `fix/parallel-index-construction`. The shared case-folded bigram implementation is already present and its semantics and snapshot compatibility must be preserved.

The applicable serial fresh-build baseline is the three-process [casefold build recording](../../bench/results/musenalm/casefold-build-ff759b225c/) at measured code revision `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. The later report-only commit did not change build code. The older `20261604be` build recording predates case-folding and cannot isolate this assignment's construction changes. The applicable run used 148,486 Musenalm records, source-metadata fingerprint `eec50dbfbfd023aad31e9c3609f4a7f14b177ecc1a022c182321ae704c188cf6` from its saved `gate.json`, the same schema and registrations, Rust 1.99.1 release, performance governor, affinity CPUs 0–31, 32 query workers, and zero source/decoded-cache budgets. Recheck these conditions before final comparison.

| Configuration | Serial wall median | Extraction/index median | Snapshot write median | Peak RSS median |
| --- | ---: | ---: | ---: | ---: |
| ASCII, disabled | 11.958 s | See raw samples | 0 | 236.5 MiB |
| Unicode, disabled | 12.165 s | See raw samples | 0 | 242.2 MiB |
| ASCII, rebuild | 12.260 s | See raw samples | See raw samples | 339.4 MiB |
| Unicode, rebuild | 12.401 s | See raw samples | See raw samples | 346.9 MiB |

At task start, `nproc` reported 32 available CPUs, process affinity was 0–31, the governor was `performance`, and `rustc`/`cargo` were 1.99.1. These are observations, not assumptions about future benchmark runs.

Verification order: first fix unavailable matched-field evidence and durable schema identity independently; then remove linear file-membership checks; then compare chunked gram maps with an independent constructor before parallel scheduling. Parallel builds must match one-worker semantic indexes, diagnostics, record ordering and snapshot round trips. Final fresh-process benchmarks compare serial, optimized one-worker and automatic workers with the same corpus, schemas and machine policy, including single-file and mixed-size fixtures. Query performance is outside this task.

## Retained baseline schema provenance

The casefold query and build runs predate saved semantic schema manifests. Their original `run-manifest.json` and `gate.json` remain unchanged. Additional `schema-manifest.json` and `schema-provenance.json` files were added to each result directory after reconstructing the seven Musenalm schema documents from the retained fixture. Before accepting the conversion, the reconstruction matched the query run's path-sensitive SHA-256 schema fingerprint (`4941bc3e09e4a6a5837375d0ce0db16f9372b469ee2b2971914d9f9527b0957a`) and the build run's relative-path schema fingerprint (`041a1bb268a932de61e3b3ff26407ac8e02edc25eaf8e7533fd239f8d59efbdf`). Both recorded code revisions are `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. The saved canonical schema digest is `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`.

The build result also gains `registration-manifest.json`, copied from the same revision's full query run after verifying the recorded revision and schema identity. A legacy result lacking verifiable retained documents is reported as an unverifiable schema baseline, even when both original checkout directories are absent. Future runs save the exact parsed schema documents in their own result directory during benchmark setup.
