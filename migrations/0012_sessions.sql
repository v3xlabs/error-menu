CREATE TABLE auth_attempts (
    state_hash TEXT PRIMARY KEY,
    expires_at TEXT NOT NULL
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    expires_at TEXT NOT NULL
);

CREATE INDEX sessions_user_id ON sessions(user_id);
