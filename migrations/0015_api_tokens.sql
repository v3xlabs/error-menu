CREATE TABLE api_tokens (
    token_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    created_at_millis INTEGER NOT NULL,
    expires_at_millis INTEGER,
    UNIQUE (user_id, name)
);

CREATE INDEX api_tokens_user_id ON api_tokens(user_id);
CREATE INDEX api_tokens_expires_at_millis ON api_tokens(expires_at_millis);
