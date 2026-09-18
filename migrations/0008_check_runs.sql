CREATE TABLE check_runs (
    id              INTEGER PRIMARY KEY,
    snapshot_id     INTEGER NOT NULL REFERENCES snapshots(id),
    name            TEXT NOT NULL,
    status          TEXT NOT NULL,
    conclusion      TEXT,
    url             TEXT,
    log_excerpt_ref TEXT
);

CREATE INDEX check_runs_by_snapshot ON check_runs(snapshot_id, id);
