# Direct snapshot access format: retired

This document described a persisted snapshot access format used by the
completed `direct-snapshot-access` task. The format and its readers have been
removed. Objekte builds its index from source files into RAM on every opening
and has no index save, load, cache or mmap path.

See [search-core](../search-core.md) for the current in-memory lifecycle and
the historical specification at
[objekte_direct_snapshot_access_specification.md](../ai/done/objekte_direct_snapshot_access_specification.md).
The original document remains available in Git history.
