import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Job, Project } from "../api/projects";
import { listJobs, listProjects } from "../api/projects";
import { QueueStateBadge } from "../components/QueueState";
import { nextQueueReadMs, queueState } from "../domain/job";
import { datePeriod } from "../domain/time";

type QueuePageState
  = | { phase: "loading"; }
    | { phase: "loaded"; jobs: readonly Job[]; names: Record<string, string>; }
    | { phase: "error"; message: string; };

type JobGroup = { period: string; jobs: Job[]; };

// The API lists jobs by when they were queued, but a row shows when it finished, so a run that
// spans midnight interleaves two days. Groups keep the order in which each period first appears.
const groupByPeriod = (jobs: readonly Job[]): readonly JobGroup[] => {
  const groups = new Map<string, JobGroup>();

  for (const job of jobs) {
    const period = datePeriod(job.finished_at ?? job.created_at);
    const group = groups.get(period);

    if (group === undefined) groups.set(period, { period, jobs: [job] });
    else group.jobs.push(job);
  }

  return Array.from(groups.values());
};

const renderJobs = (state: QueuePageState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading the queue...</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show
          when={state.jobs.length > 0}
          fallback={(
            <p class="rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500">
              The queue is empty. The schedule fills it when a project's interval elapses.
            </p>
          )}
        >
          <div class="space-y-6">
            <For each={groupByPeriod(state.jobs)}>
              {group => (
                <section class="space-y-2">
                  <h2 class="text-sm font-semibold text-slate-700 first-letter:uppercase dark:text-slate-300">{group.period}</h2>
                  <ul class="divide-y divide-hairline rounded-panel bg-surface">
                    <For each={group.jobs}>
                      {job => (
                        <li class="flex flex-wrap items-center gap-x-4 gap-y-1 px-5 py-3">
                          <a
                            href={`/projects/${job.project_id}`}
                            class="min-w-0 flex-1 truncate text-sm font-medium text-slate-900 hover:underline dark:text-slate-100"
                          >
                            {state.names[job.project_id] ?? job.project_id}
                          </a>
                          <span class="font-mono text-xs text-slate-400 dark:text-slate-500">{job.kind}</span>
                          <span class="shrink-0">
                            <QueueStateBadge state={queueState([job])} time="clock" />
                          </span>
                        </li>
                      )}
                    </For>
                  </ul>
                </section>
              )}
            </For>
          </div>
        </Show>
      );
    }
  }
};

export const QueuePage = () => {
  const [state, setState] = createSignal<QueuePageState>({ phase: "loading" });

  const load = async (): Promise<void> => {
    const [jobs, projects] = await Promise.all([listJobs(), listProjects()]);

    if (!jobs.ok) {
      setState({ phase: "error", message: jobs.message });

      return;
    }

    setState({
      phase: "loaded",
      jobs: jobs.value,
      names: names(projects.ok ? projects.value : []),
    });
  };

  createEffect(
    () => undefined,
    () => {
      void load();
    },
  );
  createEffect(
    () => {
      const current = state();

      return current.phase === "loaded" ? current.jobs : undefined;
    },
    (jobs) => {
      if (jobs === undefined) return;

      const delayMs = nextQueueReadMs(jobs);

      if (delayMs === undefined) return;

      const timer = setTimeout(() => void load(), delayMs);

      return () => clearTimeout(timer);
    },
  );

  return (
    <div class="space-y-4">
      <h1 class="text-lg font-semibold">Queue</h1>
      {renderJobs(state())}
    </div>
  );
};

const names = (projects: readonly Project[]): Record<string, string> =>
  Object.fromEntries(projects.map(project => [project.project_id, project.name]));
