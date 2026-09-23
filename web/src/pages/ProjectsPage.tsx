import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Project } from "../api/projects";
import { listProjects } from "../api/projects";
import { useAccount } from "../app/account";
import { useScope } from "../app/scope";
import { AddProjectModal } from "../components/AddProjectModal";
import { AnalyzerRing } from "../components/AnalyzerRing";
import { ProjectMark } from "../components/ProjectMark";
import { remoteLocation } from "../domain/project";
import { sinceNow } from "../domain/time";

type ProjectsState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; projects: readonly Project[]; }
    | { phase: "error"; message: string; };

type ProjectOrder = "recent" | "organization";

const ORDER_STORAGE_KEY = "error-menu-project-order";

const ORDERS: readonly { order: ProjectOrder; label: string; }[] = [
  { order: "recent", label: "Recent activity" },
  { order: "organization", label: "By organization" },
];

type OrganizationGroup = {
  organizationId: string;
  organizationName: string;
  projects: Project[];
};

const groupByOrganization = (projects: readonly Project[]): readonly OrganizationGroup[] => {
  const groups: OrganizationGroup[] = [];
  const byOrganization = new Map<string, OrganizationGroup>();

  for (const project of projects) {
    const group = byOrganization.get(project.organization_id);

    if (group === undefined) {
      const created = {
        organizationId: project.organization_id,
        organizationName: project.organization_name,
        projects: [project],
      };

      byOrganization.set(project.organization_id, created);
      groups.push(created);

      continue;
    }

    group.projects.push(project);
  }

  return groups;
};

// A project that has never moved sorts last rather than first.
const activityMs = (project: Project): number =>
  (project.last_activity_at === undefined ? 0 : Date.parse(project.last_activity_at));

const PANEL_MESSAGE = "rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500";

const ProjectRow = (properties: { project: Project; showsOrganization: boolean; }) => (
  <li>
    <a href={`/projects/${properties.project.project_id}`} class="flex items-center justify-between gap-4 px-5 py-3.5 hover:bg-raised">
      <ProjectMark project={properties.project} size={32} />
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium text-slate-900 dark:text-slate-100">{properties.project.name}</p>
        <p class="flex min-w-0 items-center gap-1.5 text-xs text-slate-500 dark:text-slate-500">
          <span class="truncate font-mono">{remoteLocation(properties.project.remote_url)}</span>
          <Show when={properties.project.remote_url.startsWith("ssh://")}>
            <span aria-hidden="true">-</span>
            <span>ssh</span>
          </Show>
          <Show when={properties.showsOrganization}>
            <span aria-hidden="true">-</span>
            <span class="shrink-0">{properties.project.organization_name}</span>
          </Show>
          <Show when={properties.project.last_activity_at}>
            {activity => (
              <>
                <span aria-hidden="true">-</span>
                <time class="shrink-0" datetime={activity()} title={new Date(activity()).toLocaleString()}>
                  {sinceNow(activity())}
                </time>
              </>
            )}
          </Show>
        </p>
      </div>
      <AnalyzerRing entries={properties.project.default_branch?.analyzers ?? []} />
    </a>
  </li>
);

const ProjectList = (properties: { projects: readonly Project[]; showsOrganization: boolean; }) => (
  <ul class="divide-y divide-hairline overflow-hidden rounded-panel bg-surface">
    <For each={properties.projects}>
      {project => <ProjectRow project={project} showsOrganization={properties.showsOrganization} />}
    </For>
  </ul>
);

const renderProjects = (state: ProjectsState, order: ProjectOrder, organizationId: string | null): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading projects...</p>;
    }
    case "anonymous": {
      return <p class={PANEL_MESSAGE}>Sign in to view projects.</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      const scoped = organizationId === null
        ? state.projects
        : state.projects.filter(project => project.organization_id === organizationId);

      if (state.projects.length === 0) return <p class={PANEL_MESSAGE}>No projects yet.</p>;

      if (scoped.length === 0) return <p class={PANEL_MESSAGE}>No projects in this organization.</p>;

      if (order === "recent") {
        return (
          <ProjectList
            projects={scoped.toSorted((left, right) => activityMs(right) - activityMs(left))}
            showsOrganization={organizationId === null}
          />
        );
      }

      return (
        <div class="space-y-6">
          <For each={groupByOrganization(scoped)}>
            {group => (
              <section class="space-y-2">
                <h2 class="text-sm font-semibold text-slate-700 dark:text-slate-300">
                  <a href={`/orgs/${group.organizationId}`} class="hover:underline">{group.organizationName}</a>
                </h2>
                <ProjectList projects={group.projects} showsOrganization={false} />
              </section>
            )}
          </For>
        </div>
      );
    }
  }
};

export const ProjectsPage = () => {
  const account = useAccount();
  const scope = useScope();
  const [state, setState] = createSignal<ProjectsState>({ phase: "loading" });
  const [order, setOrder] = createSignal<ProjectOrder>(
    localStorage.getItem(ORDER_STORAGE_KEY) === "organization" ? "organization" : "recent",
  );
  let latestLoad = 0;

  const chooseOrder = (next: ProjectOrder): void => {
    setOrder(next);
    localStorage.setItem(ORDER_STORAGE_KEY, next);
  };

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

    setState({ phase: "loaded", projects: result.value });
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
      <div class="flex items-center justify-between gap-4">
        <h1 class="text-lg font-semibold">Projects</h1>
        <Show when={account.user()}>
          {user => (
            <div class="flex items-center gap-3">
              <div role="group" aria-label="Order projects" class="flex gap-0.5">
                <For each={ORDERS}>
                  {option => (
                    <button
                      type="button"
                      aria-pressed={order() === option.order ? "true" : "false"}
                      onClick={() => chooseOrder(option.order)}
                      class="rounded-control px-2.5 py-1.5 text-sm text-slate-500 hover:text-slate-900 aria-pressed:bg-raised aria-pressed:font-medium aria-pressed:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100 dark:aria-pressed:text-slate-100"
                    >
                      {option.label}
                    </button>
                  )}
                </For>
              </div>
              <Show when={user().role !== "guest"}>
                <AddProjectModal onCreated={() => void load()} />
              </Show>
            </div>
          )}
        </Show>
      </div>
      {renderProjects(state(), order(), scope.organizationId())}
    </div>
  );
};
