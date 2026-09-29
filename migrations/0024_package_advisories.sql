-- An advisory is published against one package version, so it hangs off the same
-- coordinate as the rest of the facts and is replaced with them on every read. The counts
-- it replaces were a second answer to the same question.
CREATE TABLE package_advisories (
    ecosystem   TEXT NOT NULL,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL,
    advisory_id TEXT NOT NULL,
    -- Space separated: advisory identifiers never contain a space.
    aliases     TEXT NOT NULL,
    severity    TEXT NOT NULL,
    summary     TEXT,
    fixed_in    TEXT,
    PRIMARY KEY (ecosystem, name, version, advisory_id)
);

ALTER TABLE package_facts DROP COLUMN vulnerabilities;

ALTER TABLE package_facts DROP COLUMN vulnerabilities_high;

-- Every cached row was read without its advisories. The cache refills itself.
DELETE FROM package_facts;
