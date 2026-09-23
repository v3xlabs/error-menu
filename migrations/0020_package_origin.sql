-- Where a package resolved from, and what the lockfile claims its bytes hash to. Two
-- columns for the origin because the reference means a different thing per kind, and one
-- column would have to be parsed back apart by every reader. A finding recorded before this
-- migration has neither, so both are nullable and a reader that finds NULL draws no link
-- rather than guessing one.
ALTER TABLE findings ADD COLUMN origin_kind TEXT;

ALTER TABLE findings ADD COLUMN origin_ref TEXT;

ALTER TABLE findings ADD COLUMN package_integrity TEXT;
