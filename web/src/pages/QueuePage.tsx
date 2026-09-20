import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Job, Project } from "../api/projects";
import { listJobs, listProjects } from "../api/projects";
import { QueueStateBadge } from "../components/QueueState";
import { nextQueueReadMs, queueState } from "../domain/job";

type QueuePageState
  = | { phase: "loading"; }
    | { phase: "loaded"; jobs: readonly Job[]; names: Record<string, string>; }
    | { phase: "error"; message: string; };

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
            <p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">
              The queue is empty. The schedule fills it when a project's interval elapses.
            </p>
          )}
        >
          <ul class="divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For each={state.jobs}>
              {job => (
                <li class="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5">
                  <a
                    href={`/projects/${job.project_id}`}
                    class="min-w-0 flex-1 truncate text-sm font-medium text-slate-900 hover:underline dark:text-slate-100"
                  >
                    {state.names[job.project_id] ?? job.project_id}
                  </a>
                  <span class="font-mono text-xs text-slate-400 dark:text-slate-500">{job.kind}</span>
                  <span class="shrink-0">
                    <QueueStateBadge state={queueState([job])} />
                  </span>
                </li>
              )}
            </For>
          </ul>
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
      <div class="flex items-center justify-between gap-4">
        <h1 class="text-lg font-semibold">Queue</h1>
        <p class="text-xs text-slate-500 dark:text-slate-400">
          Every job the schedule has run, newest first.
        </p>
      </div>
      {renderJobs(state())}
    </div>
  );
};

const names = (projects: readonly Project[]): Record<string, string> =>
  Object.fromEntries(projects.map(project => [project.project_id, project.name]));
