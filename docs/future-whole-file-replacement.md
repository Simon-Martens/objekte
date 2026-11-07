# Future whole-file replacement boundary

Runtime updates are deferred. Today, `Repository::open` builds private collection
states and publishes them together only after every selected file has been
framed, parsed, indexed, and validated. A later open constructs a new repository;
old prepared and verified caches remain attached to the old instance.

For a future supplied complete file, the collection's `FileRecordIndex` gives
all old record slots for that file without reading its old bytes. The reader
would frame and extract the complete replacement once, even if it changes from
one record to many, many to none, or moves every range. A failed extraction
must abort publication. The replacement unit is initially a collection
generation, so all old postings, raw/custom bigrams, sort keys, coverage, and
statistics for that collection can be rebuilt together. If several collections
share the file, their replacement generations must publish atomically. Deletion
is a separate operation that removes the file and all its memberships; it is
not equivalent to a valid empty replacement.

The current immutable record slots are generation-specific. Any later partial
updater must suppress old slots before exposing new ones, maintain typed
postings including presence/null/length entries, remove obsolete bigram
contributions, update sorted keys and statistics, and invalidate cache keys by
generation and binding version. Direct insertion/removal or replaceable
partitions can be chosen after measurement. Compact Roaring containers and
contiguous inventories remain useful; the present global bigram maps and sorted
columns would need replacement or bounded deltas for efficient partial writes.
No overlay, watcher, transaction log, or compactor is implemented now.

The live source path does not retain old content. Verification and payload reads
check the opened file and path before and after access against recorded modified
time and size, rejecting stale reads. Those two metadata values cannot detect a
same-size edit that restores the prior modification time; no per-query hashing
is performed. Cached locators carry their original revision and need reopening
against a new repository for current content.
