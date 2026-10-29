CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS inputs (
    path TEXT PRIMARY KEY,
    digest TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS input_collections (
    path TEXT NOT NULL,
    collection TEXT NOT NULL,
    PRIMARY KEY (path, collection)
) WITHOUT ROWID;
