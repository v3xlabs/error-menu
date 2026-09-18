ALTER TABLE users ADD COLUMN access_role TEXT NOT NULL DEFAULT 'guest' CHECK (access_role IN ('guest', 'member', 'admin'));
UPDATE users SET access_role = role;

CREATE TABLE project_members (
    project_id INTEGER NOT NULL REFERENCES projects(id),
    user_id INTEGER NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('viewer', 'operator', 'owner')),
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX project_members_user_id ON project_members(user_id);

INSERT INTO project_members (project_id, user_id, role)
SELECT projects.id, users.id, 'owner'
FROM projects
JOIN users ON users.access_role = 'admin';
