CREATE TABLE invitations (id INTEGER PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, email TEXT NOT NULL, role TEXT NOT NULL CHECK(role IN ('user','editor','administrator')), expires_at INTEGER NOT NULL, redeemed INTEGER NOT NULL DEFAULT 0, revoked INTEGER NOT NULL DEFAULT 0);
CREATE TABLE resets (token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0);
CREATE INDEX resets_user ON resets(user_id);
PRAGMA user_version=2;
