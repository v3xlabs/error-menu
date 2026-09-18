ALTER TABLE projects ADD COLUMN description TEXT;
ALTER TABLE projects ADD COLUMN icon_light_path TEXT;
ALTER TABLE projects ADD COLUMN icon_dark_path TEXT;

ALTER TABLE snapshots ADD COLUMN signature_present INTEGER NOT NULL DEFAULT 0;
ALTER TABLE snapshots ADD COLUMN signature_verified INTEGER;
ALTER TABLE snapshots ADD COLUMN signature_signer TEXT;
ALTER TABLE snapshots ADD COLUMN signature_reason TEXT;
