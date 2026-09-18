-- The queue lives in the same database as the results, so storing what a job produced and
-- marking the job done is one transaction that cannot half happen. There is no subject_id
-- column: every job kind today is project scoped, and a column nothing writes is a lie
-- about what the queue can do.
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    kind TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    claimed_by TEXT,
    lease_expires_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    available_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE INDEX jobs_claimable ON jobs (state, available_at, priority DESC, id);
CREATE INDEX jobs_by_project ON jobs (project_id, id DESC);

-- How often a project is polled, and when it was last put in the queue. A project holds its
-- own schedule because a repository that changes twice a year does not deserve the same
-- attention as one that changes twice an hour.
ALTER TABLE projects ADD COLUMN watch_interval_seconds INTEGER NOT NULL DEFAULT 900;
ALTER TABLE projects ADD COLUMN last_enqueued_at TEXT;
