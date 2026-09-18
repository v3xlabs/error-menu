import type { Job } from "../api/projects";

export type QueueState
  = | { phase: "idle"; }
    | { phase: "queued"; retryOf: string | undefined; }
    | { phase: "running"; }
    | { phase: "done"; at: string; }
    | { phase: "failed"; error: string | undefined; attempts: number; };

// The newest job is the answer: an older one is history a reader did not ask for. A queued
// job that already carries an error is a retry, which reads differently from a first run.
export const queueState = (jobs: readonly Job[]): QueueState => {
  const newest = jobs.at(0);

  if (newest === undefined) return { phase: "idle" };

  switch (newest.state) {
    case "running": {
      return { phase: "running" };
    }
    case "queued": {
      return { phase: "queued", retryOf: newest.last_error };
    }
    case "failed": {
      return { phase: "failed", error: newest.last_error, attempts: newest.attempts };
    }
    case "done": {
      return { phase: "done", at: newest.finished_at ?? newest.created_at };
    }
  }
};

export const isMoving = (state: QueueState): boolean =>
  state.phase === "running" || state.phase === "queued";
