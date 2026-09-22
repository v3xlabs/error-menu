CREATE TABLE organizations (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT
);

CREATE TABLE organization_members (
    organization_id INTEGER NOT NULL REFERENCES organizations(id),
    user_id INTEGER NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('viewer', 'operator', 'owner')),
    PRIMARY KEY (organization_id, user_id)
);

CREATE INDEX organization_members_user_id ON organization_members(user_id);

INSERT INTO organizations (id, name, description) VALUES (1, 'Default', NULL);

INSERT INTO organization_members (organization_id, user_id, role)
SELECT 1, users.id, 'owner' FROM users WHERE users.access_role = 'admin';

-- No REFERENCES clause, and not for want of one. SQLite refuses to add a column that
-- carries one unless its default is NULL, and a nullable organization would leave a
-- project that no listing can reach. NOT NULL is the invariant every read here depends
-- on, so the column keeps that and gives up the declared key.
ALTER TABLE projects ADD COLUMN organization_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX projects_organization_id ON projects(organization_id);
