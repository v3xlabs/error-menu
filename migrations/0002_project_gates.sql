CREATE TABLE project_gates (
    project_id INTEGER NOT NULL REFERENCES projects (id),
    gate       TEXT NOT NULL,
    PRIMARY KEY (project_id, gate)
);
