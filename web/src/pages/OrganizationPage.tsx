import { useNavigate, useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { Organization } from "../api/organizations";
import { deleteOrganization, describeOrganization, readOrganization } from "../api/organizations";
import type { Project } from "../api/projects";
import { listProjects } from "../api/projects";
import { OrganizationMembers } from "../components/OrganizationMembers";
import { ProjectMark } from "../components/ProjectMark";

type OrganizationState
  = | { phase: "loading"; }
    | { phase: "loaded"; organization: Organization; }
    | { phase: "error"; message: string; };
type ProjectsState
  = | { phase: "loading"; }
    | { phase: "loaded"; projects: readonly Project[]; }
    | { phase: "error"; message: string; };
type SaveState = { phase: "ready"; } | { phase: "saving"; } | { phase: "error"; message: string; };
type DeleteState = { phase: "ready"; } | { phase: "deleting"; } | { phase: "error"; message: string; };

const FIELD = "w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";

const renderProjects = (state: ProjectsState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading projects...</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show when={state.projects.length > 0} fallback={<p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">No projects in this organization yet.</p>}>
          <ul class="divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For each={state.projects}>
              {project => (
                <li>
                  <a href={`/projects/${project.project_id}`} class="flex items-center justify-between gap-4 px-4 py-3 hover:bg-slate-100 dark:hover:bg-slate-900">
                    <ProjectMark project={project} size={32} />
                    <div class="min-w-0 flex-1">
                      <p class="text-sm font-medium text-slate-900 dark:text-slate-100">{project.name}</p>
                      <p class="truncate text-xs text-slate-500 dark:text-slate-500">{project.remote_url}</p>
                    </div>
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

const OrganizationSettings = (properties: {
  organization: Organization;
  onSaved: (organization: Organization) => void;
}) => {
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [saveState, setSaveState] = createSignal<SaveState>({ phase: "ready" });
  const [deleteState, setDeleteState] = createSignal<DeleteState>({ phase: "ready" });
  const [isArmed, setIsArmed] = createSignal(false);
  const navigate = useNavigate();

  createEffect(
    () => ({
      name: properties.organization.name,
      description: properties.organization.description ?? "",
    }),
    (organization) => {
      setName(organization.name);
      setDescription(organization.description);
    },
  );

  const saveError = (): string | null => {
    const current = saveState();

    return current.phase === "error" ? current.message : null;
  };

  const save = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const organizationName = name().trim();

    if (organizationName.length === 0) {
      setSaveState({ phase: "error", message: "Enter an organization name." });

      return;
    }

    const organizationDescription = description().trim();

    setSaveState({ phase: "saving" });

    const result = await describeOrganization(
      properties.organization.organization_id,
      organizationDescription.length === 0
        ? { name: organizationName }
        : { name: organizationName, description: organizationDescription },
    );

    if (!result.ok) {
      setSaveState({ phase: "error", message: result.message });

      return;
    }

    setSaveState({ phase: "ready" });
    properties.onSaved(result.value);
  };

  const deleteError = (): string | null => {
    const current = deleteState();

    return current.phase === "error" ? current.message : null;
  };

  const remove = async (): Promise<void> => {
    setDeleteState({ phase: "deleting" });

    const result = await deleteOrganization(properties.organization.organization_id);

    if (!result.ok) {
      setDeleteState({ phase: "error", message: result.message });
      setIsArmed(false);

      return;
    }

    navigate("/orgs");
  };

  return (
    <section class="border-t border-slate-200 pt-6 dark:border-slate-800">
      <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Settings</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">Rename this organization or rewrite what it is for.</p>
      <form class="mt-4 space-y-3" onSubmit={event => void save(event)}>
        <div class="space-y-1.5">
          <label for="organization-name" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
            Name
          </label>
          <input
            id="organization-name"
            type="text"
            value={name()}
            disabled={saveState().phase === "saving"}
            onInput={event => setName(event.currentTarget.value)}
            class={FIELD}
          />
        </div>
        <div class="space-y-1.5">
          <label for="organization-description" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
            Description
          </label>
          <input
            id="organization-description"
            type="text"
            value={description()}
            disabled={saveState().phase === "saving"}
            onInput={event => setDescription(event.currentTarget.value)}
            placeholder="What this organization owns, in a sentence."
            class={FIELD}
          />
        </div>
        <Show when={saveError()}>
          {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
        </Show>
        <button
          type="submit"
          disabled={saveState().phase === "saving"}
          class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
        >
          {saveState().phase === "saving" ? "Saving..." : "Save"}
        </button>
      </form>
      <OrganizationMembers organizationId={properties.organization.organization_id} />
      <h2 class="mt-6 text-base font-semibold text-slate-900 dark:text-slate-100">Delete</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
        An organization goes only once it is empty. Move or delete every project inside it first, and its grants go with it.
      </p>
      <Show when={deleteError()}>
        {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <div class="mt-4 flex items-center gap-2">
        <button
          type="button"
          disabled={deleteState().phase === "deleting"}
          onClick={() => (isArmed() ? void remove() : setIsArmed(true))}
          class="rounded-md border border-red-600 px-3 py-1.5 text-sm font-medium text-red-600 hover:bg-red-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-red-400 dark:text-red-400 dark:hover:bg-red-950"
        >
          <Switch fallback="Delete organization">
            <Match when={deleteState().phase === "deleting"}>Deleting...</Match>
            <Match when={isArmed()}>{`Delete ${properties.organization.name} for good`}</Match>
          </Switch>
        </button>
        <Show when={isArmed() && deleteState().phase !== "deleting"}>
          <button
            type="button"
            onClick={() => setIsArmed(false)}
            class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-100 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
          >
            Cancel
          </button>
        </Show>
      </div>
    </section>
  );
};

export const OrganizationPage = () => {
  const routeParameters = useParams<{ organizationId: string; }>();
  const [state, setState] = createSignal<OrganizationState>({ phase: "loading" });
  const [projectsState, setProjectsState] = createSignal<ProjectsState>({ phase: "loading" });

  // There is no per-organization project endpoint, so the whole readable list is filtered here.
  const load = async (organizationId: string): Promise<void> => {
    setState({ phase: "loading" });
    setProjectsState({ phase: "loading" });

    const [organization, projects] = await Promise.all([readOrganization(organizationId), listProjects()]);

    setState(organization.ok
      ? { phase: "loaded", organization: organization.value }
      : { phase: "error", message: organization.message });
    setProjectsState(projects.ok
      ? { phase: "loaded", projects: projects.value.filter(project => project.organization_id === organizationId) }
      : { phase: "error", message: projects.message });
  };

  createEffect(
    () => routeParameters.organizationId,
    (organizationId) => {
      void load(organizationId);
    },
  );

  const renderOrganization = (organization: Organization): JSX.Element => (
    <div class="space-y-6">
      <header class="space-y-1">
        <h1 class="text-lg font-semibold">{organization.name}</h1>
        <Show when={organization.description}>
          {description => <p class="text-sm text-slate-600 dark:text-slate-300">{description()}</p>}
        </Show>
        <p class="text-sm text-slate-500 dark:text-slate-500">
          {organization.project_count === 1 ? "1 project" : `${organization.project_count} projects`}
        </p>
      </header>
      <section class="space-y-3">
        <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Projects</h2>
        {renderProjects(projectsState())}
      </section>
      <Show when={organization.viewer_role === "owner"}>
        <OrganizationSettings
          organization={organization}
          onSaved={saved => setState({ phase: "loaded", organization: saved })}
        />
      </Show>
    </div>
  );

  const render = (current: OrganizationState): JSX.Element => {
    switch (current.phase) {
      case "loading": {
        return <p class="text-sm text-slate-500 dark:text-slate-500">Loading organization...</p>;
      }
      case "error": {
        return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{current.message}</p>;
      }
      case "loaded": {
        return renderOrganization(current.organization);
      }
    }
  };

  return <>{render(state())}</>;
};
