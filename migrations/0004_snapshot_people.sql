ALTER TABLE snapshots ADD COLUMN forge_merge_commit TEXT;

CREATE TABLE snapshot_people (
    id INTEGER PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    role TEXT NOT NULL,
    person_name TEXT,
    email TEXT,
    login TEXT,
    avatar_url TEXT,
    identity TEXT NOT NULL
);

CREATE INDEX snapshot_people_snapshot ON snapshot_people(snapshot_id);
CREATE INDEX snapshot_people_identity ON snapshot_people(identity);
