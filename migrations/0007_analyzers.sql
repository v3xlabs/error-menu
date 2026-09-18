ALTER TABLE project_gates RENAME TO project_analyzers;
ALTER TABLE project_analyzers RENAME COLUMN gate TO analyzer;
ALTER TABLE runs RENAME COLUMN gate TO analyzer;
ALTER TABLE issues RENAME COLUMN gate TO analyzer;
