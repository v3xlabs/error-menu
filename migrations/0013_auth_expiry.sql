DROP TABLE auth_attempts;
DROP TABLE sessions;

CREATE TABLE auth_attempts (
    state_hash TEXT PRIMARY KEY,
    expires_at_millis INTEGER NOT NULL
);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    expires_at_millis INTEGER NOT NULL
);

CREATE INDEX auth_attempts_expires_at_millis ON auth_attempts(expires_at_millis);
CREATE INDEX sessions_expires_at_millis ON sessions(expires_at_millis);
CREATE INDEX sessions_user_id ON sessions(user_id);
