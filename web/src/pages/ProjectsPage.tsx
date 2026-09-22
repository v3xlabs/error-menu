import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Project } from "../api/projects";
import { listProjects } from "../api/projects";
import { useAccount } from "../app/account";
import { AddProjectModal } from "../components/AddProjectModal";
import { AnalyzerRing } from "../components/AnalyzerRing";
import { ProjectMark } from "../components/ProjectMark";

type ProjectsState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; projects: readonly Project[]; }
    | { phase: "error"; message: string; };

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

const renderProjects = (state: ProjectsState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading projects...</p>;
    }
    case "anonymous": {
      return <p class="rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500">Sign in to view projects.</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show when={state.projects.length > 0} fallback={<p class="rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500">No projects yet.</p>}>
          <div class="space-y-6">
            <For each={groupByOrganization(state.projects)}>
              {group => (
                <section class="space-y-2">
                  <h2 class="text-sm font-semibold text-slate-700 dark:text-slate-300">
                    <a href={`/orgs/${group.organizationId}`} class="hover:underline">{group.organizationName}</a>
                  </h2>
                  <ul class="divide-y divide-hairline overflow-hidden rounded-panel bg-surface">
                    <For each={group.projects}>
                      {project => (
                        <li>
                          <a href={`/projects/${project.project_id}`} class="flex items-center justify-between gap-4 px-5 py-3.5 hover:bg-raised">
                            <ProjectMark project={project} size={32} />
                            <div class="min-w-0 flex-1">
                              <p class="text-sm font-medium text-slate-900 dark:text-slate-100">{project.name}</p>
                              <p class="truncate text-xs text-slate-500 dark:text-slate-500">{project.remote_url}</p>
                            </div>
                            <Show
                              when={project.default_branch}
                              fallback={<span class="text-xs text-slate-500 dark:text-slate-500">No default-branch scan</span>}
                            >
                              {analysis => <AnalyzerRing entries={analysis().analyzers} />}
                            </Show>
                          </a>
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
