import type { JSX } from "@solidjs/web";
import { VsQuestion } from "solid-icons/vs";
import { createEffect, createSignal, For, Show } from "solid-js";

import { listAnalyses, listProjects } from "../api/projects";
import type { components } from "../api/schema.gen";
import { useAccount } from "../app/account";
import { AddProjectModal } from "../components/AddProjectModal";
import { Gauge } from "../components/Gauge";
import { ProjectMark } from "../components/ProjectMark";
import { runTone } from "../domain/analysis";
import { analyzerByRunId } from "../domain/analyzer";

type Project = components["schemas"]["ProjectOutput"];
type Analysis = components["schemas"]["AnalysisOutput"];
type ProjectSummary = { project: Project; defaultBranch: Analysis | undefined; message: string | undefined; };
type ProjectsState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; projects: readonly ProjectSummary[]; }
    | { phase: "error"; message: string; };

// An analyzer the client does not know yet still gets a gauge, because the backend owns the
// list and can ship one before this build does.
const analyzerIcon = (runId: string): JSX.Element => {
  const analyzer = analyzerByRunId(runId);

  return analyzer === undefined ? <VsQuestion size={12} /> : <analyzer.icon size={12} />;
};

const analyzerLabel = (runId: string): string => analyzerByRunId(runId)?.label ?? runId;

const renderProjects = (state: ProjectsState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading projects...</p>;
    }
    case "anonymous": {
      return <p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">Sign in to view projects.</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show when={state.projects.length > 0} fallback={<p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">No projects yet.</p>}>
          <ul class="divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For each={state.projects}>
              {summary => (
                <li>
                  <a href={`/projects/${summary.project.project_id}`} class="flex items-center justify-between gap-4 px-4 py-3 hover:bg-slate-100 dark:hover:bg-slate-900">
                    <ProjectMark project={summary.project} size={32} />
                    <div class="min-w-0 flex-1">
                      <p class="text-sm font-medium text-slate-900 dark:text-slate-100">{summary.project.name}</p>
                      <p class="truncate text-xs text-slate-500 dark:text-slate-500">{summary.project.remote_url}</p>
                    </div>
                    <Show
                      when={summary.message === undefined}
                      fallback={<span class="shrink-0 text-xs text-red-600 dark:text-red-400">{summary.message}</span>}
                    >
                      <Show
                        when={summary.defaultBranch}
                        fallback={<span class="text-xs text-slate-500 dark:text-slate-500">No default-branch scan</span>}
                      >
                        {analysis => (
                          <span class="flex shrink-0 items-center gap-1.5">
                            <For each={analysis().analyzers} fallback={<span class="text-xs text-slate-500 dark:text-slate-500">No default-branch scan</span>}>
                              {analyzer => (
                                <Gauge
                                  tone={runTone(analyzer)}
                                  icon={analyzerIcon(analyzer.analyzer)}
                                  value={analyzer.finding_count}
                                  label={`${analyzerLabel(analyzer.analyzer)}: ${runTone(analyzer)}, ${analyzer.finding_count} ${analyzer.finding_count === 1 ? "finding" : "findings"}`}
                                />
                              )}
                            </For>
                          </span>
                        )}
                      </Show>
                    </Show>
                  </a>
                </li>
              )}
            </For>
          </ul>
        </Show>
      );
    }
  }
};

export const ProjectsPage = () => {
  const account = useAccount();
  const [state, setState] = createSignal<ProjectsState>({ phase: "loading" });
  let latestLoad = 0;

  const load = async (): Promise<void> => {
    const userId = account.user()?.user_id;

    if (userId === undefined) return;

    const loadId = ++latestLoad;
    const result = await listProjects();

    if (loadId !== latestLoad || account.user()?.user_id !== userId) return;

    if (!result.ok) {
      setState({ phase: "error", message: result.message });

      return;
    }

    const projects = await Promise.all(result.value.map(async (project) => {
      const analyses = await listAnalyses(project.project_id);

      return {
        project,
        defaultBranch: analyses.ok ? analyses.value.find(analysis => analysis.subject.kind === "branch") : undefined,
        message: analyses.ok ? undefined : analyses.message,
      };
    }));

    if (loadId !== latestLoad || account.user()?.user_id !== userId) return;

    setState({ phase: "loaded", projects });
  };

  createEffect(
    () => account.state(),
    (accountState) => {
      switch (accountState.phase) {
        case "loaded": {
          void load();

          return;
        }
        case "anonymous": {
          latestLoad += 1;
          setState({ phase: "anonymous" });

          return;
        }
        case "error": {
          latestLoad += 1;
          setState({ phase: "error", message: accountState.message });

          return;
        }
        case "loading": {
          latestLoad += 1;
          setState({ phase: "loading" });
        }
      }
    },
  );

  return (
    <div class="space-y-6">
      <div class="flex items-center justify-between">
        <h1 class="text-lg font-semibold">Projects</h1>
        <Show when={account.user()}>
          {user => (
            <Show when={user().role !== "guest"}>
              <AddProjectModal onCreated={() => void load()} />
            </Show>
          )}
        </Show>
      </div>
      {renderProjects(state())}
    </div>
  );
};
