-- A job names what it is about and what it uses. `subject` is the identity a second request
-- for the same work collapses into, which for a package is its coordinate and belongs to no
-- project, so `project_id` becomes optional. The table is rebuilt because SQLite cannot drop
-- a NOT NULL constraint.
CREATE TABLE jobs_next (
    id INTEGER PRIMARY KEY,
    project_id INTEGER REFERENCES projects (id),
    kind TEXT NOT NULL,
    subject TEXT NOT NULL,
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

-- A per-project facts job has no successor of the same shape: facts are now fetched one
-- package per job, and the sweep finds every package still without an answer.
INSERT INTO jobs_next
    (id, project_id, kind, subject, priority, state, claimed_by, lease_expires_at, attempts,
     last_error, available_at, created_at, finished_at)
SELECT id, project_id, kind, CAST(project_id AS TEXT), priority, state, claimed_by,
       lease_expires_at, attempts, last_error, available_at, created_at, finished_at
FROM jobs WHERE kind = 'discover';

DROP TABLE jobs;

ALTER TABLE jobs_next RENAME TO jobs;

CREATE INDEX jobs_claimable ON jobs (state, available_at, priority DESC, id);
CREATE INDEX jobs_by_project ON jobs (project_id, id DESC);
CREATE INDEX jobs_by_subject ON jobs (kind, subject, state);

-- What a waiting or running job holds while it runs, and how many jobs may hold it at once.
-- The capacity is written from the code's definition when the job is queued, so the claim can
-- decide in SQL alone. Rows go when the job finishes: only live jobs compete for anything.
CREATE TABLE job_resources (
    job_id INTEGER NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    resource TEXT NOT NULL,
    capacity INTEGER NOT NULL,
    PRIMARY KEY (job_id, resource)
);

CREATE INDEX job_resources_by_resource ON job_resources (resource, job_id);

-- A resource that answered "not now". Every job that needs it waits until `blocked_until`;
-- every other job carries on.
CREATE TABLE resource_blocks (
    resource TEXT PRIMARY KEY,
    blocked_until TEXT NOT NULL,
    reason TEXT NOT NULL
);

-- A job that is done or failed holds nothing. Released in the same statement that ends it,
-- so no crash can leave a finished job occupying a slot.
CREATE TRIGGER job_resources_release AFTER UPDATE OF state ON jobs
WHEN NEW.state IN ('done', 'failed')
BEGIN
    DELETE FROM job_resources WHERE job_id = NEW.id;
END;

INSERT INTO job_resources (job_id, resource, capacity)
SELECT id, 'mirror:' || project_id, 1 FROM jobs WHERE state IN ('queued', 'running');

INSERT INTO job_resources (job_id, resource, capacity)
SELECT id, 'discovery', 2 FROM jobs WHERE state IN ('queued', 'running');

-- A fetched version queues an audit for every project that names it, which is a lookup by
-- coordinate on every fetch.
CREATE INDEX findings_by_package ON findings (ecosystem, package_name, package_version);
