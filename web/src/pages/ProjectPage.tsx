import { useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, Show } from "solid-js";

import type { Analysis, Job } from "../api/projects";
import { discoverProject, listAnalyses, listJobs, readProject } from "../api/projects";
import type { components } from "../api/schema.gen";
import { ProjectDashboard } from "../components/ProjectDashboard";
import { ProjectHeader } from "../components/ProjectHeader";
import { forgeKind } from "../domain/analysis";
import { isMoving, queueState } from "../domain/job";

type Project = components["schemas"]["ProjectOutput"];
type ProjectState = { phase: "loading"; } | { phase: "loaded"; project: Project; } | { phase: "error"; message: string; };
type HistoryState = { phase: "loading"; } | { phase: "loaded"; analyses: readonly Analysis[]; } | { phase: "error"; message: string; };
type DiscoverState = { phase: "ready"; } | { phase: "running"; } | { phase: "error"; message: string; };

const QUEUE_POLL_MS = 4000;

export const ProjectPage = () => {
  const [state, setState] = createSignal<ProjectState>({ phase: "loading" });
  const [historyState, setHistoryState] = createSignal<HistoryState>({ phase: "loading" });
  const [discoverState, setDiscoverState] = createSignal<DiscoverState>({ phase: "ready" });
  const routeParameters = useParams<{ projectId: string; }>();
  const [jobs, setJobs] = createSignal<readonly Job[]>([]);
  const [queueError, setQueueError] = createSignal<string | null>(null);

  const reload = async (projectId: string): Promise<void> => {
    const result = await readProject(projectId);

    setState(result.ok
      ? { phase: "loaded", project: result.value }
      : { phase: "error", message: result.message });
  };
  const loadAnalyses = async (projectId: string): Promise<void> => {
    const result = await listAnalyses(projectId);

    setHistoryState(result.ok ? { phase: "loaded", analyses: result.value } : { phase: "error", message: result.message });
  };
  const discover = async (projectId: string): Promise<void> => {
    setDiscoverState({ phase: "running" });

    const result = await discoverProject(projectId);

    setDiscoverState(result.ok ? { phase: "ready" } : { phase: "error", message: result.message });
    await loadAnalyses(projectId);
  };
  // A finished scan changes what the analyses read, so the queue read is what tells the
  // page to look again.
  const loadJobs = async (projectId: string): Promise<void> => {
    const result = await listJobs(projectId);

    if (!result.ok) {
      setQueueError(result.message);

      return;
    }

    const wasMoving = isMoving(queueState(jobs()));

    setQueueError(null);
    setJobs(result.value);

    if (wasMoving && !isMoving(queueState(result.value))) await loadAnalyses(projectId);
  };
  const analyses = (): readonly Analysis[] => {
    const current = historyState();

    return current.phase === "loaded" ? current.analyses : [];
  };
  const historyError = (): string | null => {
    const current = historyState();

    return current.phase === "error" ? current.message : null;
  };
  const discoverError = (): string | null => {
    const current = discoverState();

    return current.phase === "error" ? current.message : null;
  };

  createEffect(
    () => routeParameters.projectId,
    (projectId) => {
      void readProject(projectId).then((result) => {
        setState(result.ok ? { phase: "loaded", project: result.value } : { phase: "error", message: result.message });
      });
      void loadAnalyses(projectId);
      void loadJobs(projectId);
    },
  );

  createEffect(
    () => [routeParameters.projectId, isMoving(queueState(jobs()))] as const,
    ([projectId, moving]) => {
      if (!moving) return;

      const timer = setInterval(() => void loadJobs(projectId), QUEUE_POLL_MS);

      return () => clearInterval(timer);
    },
  );

  const renderProject = (project: Project): JSX.Element => (
    <div class="space-y-6">
      <ProjectHeader
        project={project}
        isDiscovering={discoverState().phase === "running"}
        queue={queueState(jobs())}
        onDiscover={() => void discover(project.project_id)}
        onAnalysed={() => void loadAnalyses(project.project_id)}
        onSaved={() => void reload(project.project_id)}
      />
      <Show when={queueError()}>{message => <p class="text-sm text-red-600 dark:text-red-400">{message()}</p>}</Show>
      <Show when={historyError()}>{message => <p class="text-sm text-red-600 dark:text-red-400">{message()}</p>}</Show>
      <Show
        when={historyState().phase !== "loading"}
        fallback={<p class="text-sm text-slate-500 dark:text-slate-500">Reading saved analyses...</p>}
      >
        <ProjectDashboard
          projectId={project.project_id}
          projectForge={forgeKind(project.forge)}
          analyses={analyses()}
          isDiscovering={discoverState().phase === "running"}
          discoverError={discoverError()}
          onDiscover={() => void discover(project.project_id)}
          onAnalysed={() => void loadAnalyses(project.project_id)}
        />
      </Show>
    </div>
  );

  const render = (current: ProjectState): JSX.Element => {
    switch (current.phase) {
      case "loading": {
        return <p class="text-sm text-slate-500 dark:text-slate-500">Loading project...</p>;
      }
      case "error": {
        return <p class="text-sm text-red-600 dark:text-red-400">{current.message}</p>;
      }
      case "loaded": {
        return renderProject(current.project);
      }
    }
  };

  return <>{render(state())}</>;
};
