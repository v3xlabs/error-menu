-- A package version is the same package version in every project, so this is keyed by the
-- coordinate and by nothing else. Two projects that both lock eth-prices 1.1.0 pay for one
-- read, forever. A row is a cache of somebody else's database, so it carries its own expiry
-- and a status that tells a missing answer apart from a failed one.
CREATE TABLE package_facts (
    ecosystem            TEXT NOT NULL,
    name                 TEXT NOT NULL,
    version              TEXT NOT NULL,
    status               TEXT NOT NULL,
    fetched_at           TEXT NOT NULL,
    expires_at           TEXT NOT NULL,
    size_bytes           INTEGER,
    install_bytes        INTEGER,
    dependency_count     INTEGER,
    downloads_week       INTEGER,
    vulnerabilities      INTEGER,
    vulnerabilities_high INTEGER,
    license              TEXT,
    documentation        TEXT,
    repository           TEXT,
    homepage             TEXT,
    checksum             TEXT,
    withdrawn            TEXT,
    PRIMARY KEY (ecosystem, name, version)
);

CREATE INDEX package_facts_stale ON package_facts (expires_at);

-- Which snapshots the checksum audit has already answered. The audit runs after the facts
-- arrive, which is after the analysis finished, so the run row is what says it happened and
-- this index is what keeps it from happening twice.
CREATE INDEX runs_by_analyzer ON runs (analyzer, snapshot_id);
