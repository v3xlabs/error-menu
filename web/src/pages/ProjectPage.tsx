import { useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, Show } from "solid-js";

import type { Analysis, Commit, Job } from "../api/projects";
import { discoverProject, listAnalyses, listCommits, listJobs, readProject } from "../api/projects";
import type { components } from "../api/schema.gen";
import { ProjectDashboard } from "../components/ProjectDashboard";
import { ProjectHeader } from "../components/ProjectHeader";
import { forgeKind } from "../domain/analysis";
import { isMoving, nextQueueReadMs, queueState } from "../domain/job";

// Enough of the branch to see what landed lately without turning the page into a log.
const SHOWN_COMMITS = 8;

type Project = components["schemas"]["ProjectOutput"];
type ProjectState = { phase: "loading"; } | { phase: "loaded"; project: Project; } | { phase: "error"; message: string; };
type HistoryState = { phase: "loading"; } | { phase: "loaded"; analyses: readonly Analysis[]; } | { phase: "error"; message: string; };
type DiscoverState = { phase: "ready"; } | { phase: "running"; } | { phase: "error"; message: string; };

export const ProjectPage = () => {
  const [state, setState] = createSignal<ProjectState>({ phase: "loading" });
  const [historyState, setHistoryState] = createSignal<HistoryState>({ phase: "loading" });
  const [discoverState, setDiscoverState] = createSignal<DiscoverState>({ phase: "ready" });
  const routeParameters = useParams<{ projectId: string; }>();
  const [jobs, setJobs] = createSignal<readonly Job[]>([]);
  const [commits, setCommits] = createSignal<readonly Commit[]>([]);
  const [queueError, setQueueError] = createSignal<string | null>(null);
  let routeVersion = 0;
  let projectRead = 0;
  let historyRead = 0;
  let jobsRead = 0;
  let discoveryRead = 0;

  const isCurrent = (projectId: string, version: number): boolean =>
    routeParameters.projectId === projectId && routeVersion === version;

  const reload = async (projectId: string): Promise<void> => {
    if (routeParameters.projectId !== projectId) return;

    const version = routeVersion;
    const request = ++projectRead;
    const result = await readProject(projectId);

    if (request !== projectRead || !isCurrent(projectId, version)) return;

    setState(result.ok
      ? { phase: "loaded", project: result.value }
      : { phase: "error", message: result.message });
  };
  // A commit list is read from the mirror, so a project that was never discovered has no
  // mirror to read and answers 404. That is an empty history, not a page failure.
  const loadHistory = async (projectId: string): Promise<void> => {
    if (routeParameters.projectId !== projectId) return;

    const version = routeVersion;
    const request = ++historyRead;
    const commitsRequest = listCommits(projectId, SHOWN_COMMITS);
    const analyses: Analysis[] = [];
    let before: string | undefined;

    do {
      const page = await listAnalyses(projectId, { latest: true, ...(before !== undefined && { before }) });

      if (request !== historyRead || !isCurrent(projectId, version)) return;

      if (!page.ok) {
        setHistoryState({ phase: "error", message: page.message });

        return;
      }

      analyses.push(...page.value.analyses);
      before = page.value.next_before;
    } while (before !== undefined);

    const commits = await commitsRequest;

    if (request !== historyRead || !isCurrent(projectId, version)) return;

    setHistoryState({ phase: "loaded", analyses });
    setCommits(commits.ok ? commits.value : []);
  };
  const discover = async (projectId: string): Promise<void> => {
    if (routeParameters.projectId !== projectId) return;

    const version = routeVersion;
    const request = ++discoveryRead;

    setDiscoverState({ phase: "running" });

    const result = await discoverProject(projectId);

    if (request !== discoveryRead || !isCurrent(projectId, version)) return;

    setDiscoverState(result.ok ? { phase: "ready" } : { phase: "error", message: result.message });
    await loadHistory(projectId);
  };
  // A finished scan changes what the analyses read, so the queue read is what tells the
  // page to look again.
  const loadJobs = async (projectId: string): Promise<void> => {
    if (routeParameters.projectId !== projectId) return;

    const version = routeVersion;
    const request = ++jobsRead;
    const result = await listJobs(projectId);

    if (request !== jobsRead || !isCurrent(projectId, version)) return;

    if (!result.ok) {
      setQueueError(result.message);

      return;
    }

    const wasMoving = isMoving(queueState(jobs()));

    setQueueError(null);
    setJobs(result.value);

    if (wasMoving && !isMoving(queueState(result.value))) await loadHistory(projectId);
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
      ++routeVersion;
      setState({ phase: "loading" });
      setHistoryState({ phase: "loading" });
      setDiscoverState({ phase: "ready" });
      setJobs([]);
      setCommits([]);
      setQueueError(null);
      void reload(projectId);
      void loadHistory(projectId);
      void loadJobs(projectId);

      return () => {
        routeVersion += 1;
      };
    },
  );

  createEffect(
    () => [routeParameters.projectId, jobs()] as const,
    ([projectId, current]) => {
      const delayMs = nextQueueReadMs(current);

      if (delayMs === undefined) return;

      const timer = setTimeout(() => void loadJobs(projectId), delayMs);

      return () => clearTimeout(timer);
    },
  );

  const renderProject = (project: Project): JSX.Element => (
    <div class="space-y-6">
      <ProjectHeader
        project={project}
        isDiscovering={discoverState().phase === "running"}
        queue={queueState(jobs())}
        onDiscover={() => void discover(project.project_id)}
        onAnalysed={() => void loadHistory(project.project_id)}
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
          commits={commits()}
          isDiscovering={discoverState().phase === "running"}
          discoverError={discoverError()}
          onDiscover={() => void discover(project.project_id)}
          onAnalysed={() => void loadHistory(project.project_id)}
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
