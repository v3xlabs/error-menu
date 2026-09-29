-- A claim walks queued jobs in the order it takes them and stops at the first one that can
-- run. Keyed by `available_at` first, the index made SQLite sort every queued job on every
-- claim; keyed by the claim order, the walk ends at the head of the queue.
DROP INDEX jobs_claimable;

CREATE INDEX jobs_claimable ON jobs (state, priority DESC, id);
