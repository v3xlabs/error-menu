CREATE TABLE projects (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    remote_url TEXT NOT NULL
);

CREATE TABLE subjects (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES projects (id),
    kind        TEXT NOT NULL,
    subject_key TEXT NOT NULL,
    UNIQUE (project_id, kind, subject_key)
);

CREATE TABLE snapshots (
    id           INTEGER PRIMARY KEY,
    subject_id   INTEGER NOT NULL REFERENCES subjects (id),
    head         TEXT NOT NULL,
    base         TEXT,
    merge_base   TEXT,
    forge_title  TEXT,
    forge_body   TEXT,
    forge_author TEXT,
    observed_at  TEXT NOT NULL
);

CREATE INDEX snapshots_by_subject ON snapshots (subject_id, id);

CREATE TABLE runs (
    id               INTEGER PRIMARY KEY,
    snapshot_id      INTEGER NOT NULL REFERENCES snapshots (id),
    gate             TEXT NOT NULL,
    status           TEXT NOT NULL,
    status_detail    TEXT,
    compared_against INTEGER REFERENCES snapshots (id),
    started_at       TEXT NOT NULL,
    finished_at      TEXT
);

CREATE INDEX runs_by_snapshot ON runs (snapshot_id, gate, id);

-- Identity and human triage only. Whether an issue is open is a question about one
-- subject, and a subject-scoped lifecycle table is not built yet.
CREATE TABLE issues (
    id                    INTEGER PRIMARY KEY,
    project_id            INTEGER NOT NULL REFERENCES projects (id),
    gate                  TEXT NOT NULL,
    fingerprint           TEXT NOT NULL,
    fingerprint_version   INTEGER NOT NULL,
    fingerprint_canonical TEXT NOT NULL,
    triage                TEXT NOT NULL,
    triage_reason         TEXT,
    first_seen            INTEGER NOT NULL REFERENCES snapshots (id),
    last_seen             INTEGER NOT NULL REFERENCES snapshots (id),
    UNIQUE (project_id, fingerprint)
);

-- A finding carries no fingerprint of its own: it is the fingerprint of its issue, and
-- storing it twice invites the two to disagree.
CREATE TABLE findings (
    id              INTEGER PRIMARY KEY,
    run_id          INTEGER NOT NULL REFERENCES runs (id),
    issue_id        INTEGER NOT NULL REFERENCES issues (id),
    location_kind   TEXT NOT NULL,
    file_path       TEXT,
    line_start      INTEGER,
    line_end        INTEGER,
    ecosystem       TEXT,
    package_name    TEXT,
    package_version TEXT,
    severity        TEXT NOT NULL,
    confidence      REAL NOT NULL,
    attribution     TEXT NOT NULL,
    title           TEXT NOT NULL,
    detail          TEXT NOT NULL
);

CREATE INDEX findings_by_run ON findings (run_id);

CREATE INDEX findings_by_issue ON findings (issue_id);

CREATE TABLE signals (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL REFERENCES runs (id),
    signal_key  TEXT NOT NULL,
    value_kind  TEXT NOT NULL,
    score       REAL,
    flag        INTEGER,
    count_value INTEGER,
    confidence  REAL NOT NULL,
    reason      TEXT NOT NULL
);

CREATE INDEX signals_by_run ON signals (run_id);
