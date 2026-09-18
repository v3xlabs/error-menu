CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    oidc_issuer TEXT NOT NULL,
    oidc_subject TEXT NOT NULL,
    display_name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_at TEXT NOT NULL,
    last_signed_in_at TEXT NOT NULL,
    UNIQUE (oidc_issuer, oidc_subject)
);
