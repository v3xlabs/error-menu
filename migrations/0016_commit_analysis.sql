ALTER TABLE snapshots ADD COLUMN analysis_snapshot_id INTEGER REFERENCES snapshots (id);

CREATE TABLE commit_analyses (
    project_id  INTEGER NOT NULL REFERENCES projects (id),
    head        TEXT NOT NULL,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots (id),
    PRIMARY KEY (project_id, head)
);

-- Old snapshots have no completion marker. Require every configured analyzer and
-- a computed comparison range; closed metadata-only observations have no range.
WITH default_analyzers (analyzer) AS (
    VALUES ('lockfile-delta'), ('manifest-delta'), ('secret-scan'), ('link-inventory'),
           ('workflow-security'), ('repository-controls'), ('ci-check-runs'), ('repository-hygiene')
), expected AS (
    SELECT p.id AS project_id, d.analyzer
    FROM projects p CROSS JOIN default_analyzers d
    WHERE p.uses_default_analyzers = 1
    UNION ALL
    SELECT p.id, a.analyzer
    FROM projects p JOIN project_analyzers a ON a.project_id = p.id
    WHERE p.uses_default_analyzers = 0
)
INSERT INTO commit_analyses (project_id, head, snapshot_id)
SELECT s.project_id, n.head, MAX(n.id)
FROM snapshots n JOIN subjects s ON s.id = n.subject_id
WHERE n.merge_base IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM runs r WHERE r.snapshot_id = n.id
      AND (r.status NOT IN ('succeeded', 'failed', 'skipped') OR r.finished_at IS NULL)
  )
  AND NOT EXISTS (
      SELECT 1 FROM expected e WHERE e.project_id = s.project_id
      AND NOT EXISTS (
          SELECT 1 FROM runs r WHERE r.snapshot_id = n.id AND r.analyzer = e.analyzer
          AND r.status IN ('succeeded', 'failed', 'skipped') AND r.finished_at IS NOT NULL
      )
  )
GROUP BY s.project_id, n.head;
