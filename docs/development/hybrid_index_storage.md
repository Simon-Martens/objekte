# Hybrid index storage: retired

This document described the generated-index format, managed generations and
mapped readers used by the completed `hybrid-index-storage` task. That
implementation has been removed. Objekte builds its index from source files
into RAM on every opening and has no index save, load, cache or mmap path.

See [search-core](../search-core.md) for the current in-memory lifecycle and
the historical specification at
[objekte_hybrid_index_storage_implementation_plan.md](../ai/done/objekte_hybrid_index_storage_implementation_plan.md).
The original document remains available in Git history.

Index persistence uses the separate redb store described in [index-store.md](index-store.md).
