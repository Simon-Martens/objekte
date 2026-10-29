CREATE TABLE IF NOT EXISTS collections (
    name TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    schema_hash TEXT NOT NULL DEFAULT '',
    mapping_hash TEXT NOT NULL DEFAULT '',
    index_hash TEXT NOT NULL DEFAULT '',
    revision INTEGER NOT NULL DEFAULT 0,
    manifest_hash TEXT NOT NULL DEFAULT '',
    record_count INTEGER NOT NULL DEFAULT 0,
    view_hash TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS record_validation (
    collection TEXT NOT NULL,
    id TEXT NOT NULL,
    parse_status TEXT NOT NULL,
    validation_state TEXT NOT NULL,
    validation_schema_hash TEXT NOT NULL,
    issues TEXT NOT NULL,
    PRIMARY KEY (collection, id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS edges (
    source_collection TEXT NOT NULL,
    source_id TEXT NOT NULL,
    location TEXT NOT NULL,
    target_collection TEXT NOT NULL,
    target_id TEXT NOT NULL,
    PRIMARY KEY (source_collection, source_id, location, target_collection, target_id)
);

CREATE INDEX IF NOT EXISTS validation_pending
    ON record_validation (collection, id)
    WHERE validation_state = 'pending';

CREATE INDEX IF NOT EXISTS edge_target
    ON edges (target_collection, target_id, source_collection, source_id);
