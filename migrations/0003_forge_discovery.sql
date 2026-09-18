ALTER TABLE projects ADD COLUMN forge_kind TEXT NOT NULL DEFAULT 'auto';

ALTER TABLE snapshots ADD COLUMN forge_url TEXT;
ALTER TABLE snapshots ADD COLUMN forge_base_ref TEXT;
ALTER TABLE snapshots ADD COLUMN forge_head_ref TEXT;
ALTER TABLE snapshots ADD COLUMN forge_state TEXT;
