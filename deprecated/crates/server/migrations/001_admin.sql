CREATE TABLE users (
 id INTEGER PRIMARY KEY,
 email TEXT NOT NULL UNIQUE,
 display_name TEXT NOT NULL,
 password_hash TEXT NOT NULL,
 role TEXT NOT NULL CHECK(role IN ('user','editor','administrator')),
 disabled INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE sessions (
 token_hash TEXT PRIMARY KEY,
 user_id INTEGER NOT NULL REFERENCES users(id),
 csrf TEXT NOT NULL,
 expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
PRAGMA user_version = 1;
