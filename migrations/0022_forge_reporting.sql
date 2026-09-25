-- The install on GitHub gives error.menu permission to write to a repository; this switch
-- decides whether it does. It starts off, so an install that covers every repository of an
-- account turns nothing on by itself.
ALTER TABLE projects ADD COLUMN reports_to_forge INTEGER NOT NULL DEFAULT 0;

-- Which installation of the GitHub App covers a repository. A repository belongs to at most
-- one installation of an app, so the repository id is the key. Rows arrive only from signed
-- webhooks. The name is kept lowercased, because a remote may spell the owner in any case.
CREATE TABLE github_installation_repositories (
    repository_id   INTEGER PRIMARY KEY,
    installation_id INTEGER NOT NULL,
    full_name       TEXT NOT NULL
);

CREATE INDEX github_installation_repositories_by_name
    ON github_installation_repositories (full_name);
CREATE INDEX github_installation_repositories_by_installation
    ON github_installation_repositories (installation_id);

-- The check error.menu keeps on one commit of one project. A forge check belongs to a
-- commit, not to a subject, so a pull request head and the branch it lands on share one.
-- A new scan of a head whose check is already completed replaces the row with a new check.
CREATE TABLE forge_checks (
    project_id  INTEGER NOT NULL REFERENCES projects (id),
    head        TEXT NOT NULL,
    check_id    INTEGER NOT NULL,
    state       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (project_id, head)
);

CREATE INDEX forge_checks_by_check ON forge_checks (check_id);
