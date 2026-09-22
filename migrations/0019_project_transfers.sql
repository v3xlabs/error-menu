-- A transfer is the one write that destroys a fact nothing else holds: the organization a
-- project came from. Every other column on a project can be read back from the project.
--
-- The organizations are named, not referenced. An organization can be deleted once it is
-- empty, and a record that disappears when the organization it names does would fail at
-- exactly the moment it is asked. A name is also what the move meant at the time, so a
-- later rename leaves this history reading true.
CREATE TABLE project_transfers (
    id                     INTEGER PRIMARY KEY,
    project_id             INTEGER NOT NULL REFERENCES projects (id),
    from_organization_name TEXT NOT NULL,
    to_organization_name   TEXT NOT NULL,
    moved_by               INTEGER NOT NULL REFERENCES users (id),
    moved_at               TEXT NOT NULL
);

CREATE INDEX project_transfers_project_id ON project_transfers (project_id, id DESC);
