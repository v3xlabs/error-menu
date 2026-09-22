-- One reading per subject and head. Discovery recorded a head as soon as it saw it and
-- analysis recorded the same head again when it scanned it, so a change that waited a day
-- for its first scan carries a day of identical readings, all but one of them with no
-- analyzer run behind it. Those are what a reader sees as repeated "not scanned" rows.
--
-- The reading that was analysed survives, or the first one if none was. A reading that
-- has runs is never dropped: a re-scan of a head is a reading of its own.
CREATE TEMPORARY TABLE collapsed_snapshots AS
SELECT
    n.id AS duplicate,
    (
        SELECT m.id FROM snapshots m
        WHERE m.subject_id = n.subject_id AND m.head = n.head
        ORDER BY EXISTS (SELECT 1 FROM runs r WHERE r.snapshot_id = m.id) DESC, m.id
        LIMIT 1
    ) AS survivor
FROM snapshots n
WHERE NOT EXISTS (SELECT 1 FROM runs r WHERE r.snapshot_id = n.id);

DELETE FROM collapsed_snapshots WHERE duplicate = survivor;

UPDATE snapshots
SET analysis_snapshot_id = (
    SELECT survivor FROM collapsed_snapshots WHERE duplicate = analysis_snapshot_id
)
WHERE analysis_snapshot_id IN (SELECT duplicate FROM collapsed_snapshots);

DELETE FROM check_runs WHERE snapshot_id IN (SELECT duplicate FROM collapsed_snapshots);
DELETE FROM snapshot_people WHERE snapshot_id IN (SELECT duplicate FROM collapsed_snapshots);
DELETE FROM snapshots WHERE id IN (SELECT duplicate FROM collapsed_snapshots);

DROP TABLE collapsed_snapshots;
