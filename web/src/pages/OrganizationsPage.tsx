import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Organization } from "../api/organizations";
import { createOrganization, listOrganizations } from "../api/organizations";
import { useAccount } from "../app/account";

type OrganizationsState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; organizations: readonly Organization[]; }
    | { phase: "error"; message: string; };

const FIELD = "w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";

const renderOrganizations = (state: OrganizationsState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading organizations...</p>;
    }
    case "anonymous": {
      return <p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">Sign in to view organizations.</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show when={state.organizations.length > 0} fallback={<p class="rounded-lg border border-dashed border-slate-300 px-4 py-8 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">No organizations yet.</p>}>
          <ul class="divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For each={state.organizations}>
              {organization => (
                <li>
                  <a href={`/orgs/${organization.organization_id}`} class="flex items-center justify-between gap-4 px-4 py-3 hover:bg-slate-100 dark:hover:bg-slate-900">
                    <div class="min-w-0 flex-1">
                      <p class="text-sm font-medium text-slate-900 dark:text-slate-100">{organization.name}</p>
                      <Show when={organization.description}>
                        {description => <p class="truncate text-xs text-slate-500 dark:text-slate-500">{description()}</p>}
                      </Show>
                    </div>
                    <div class="shrink-0 text-right text-xs text-slate-500 dark:text-slate-500">
                      <p>{organization.project_count === 1 ? "1 project" : `${organization.project_count} projects`}</p>
                      <p>{`Your role: ${organization.viewer_role}`}</p>
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

const CreateOrganizationForm = (properties: { onCreated: (organization: Organization) => void; }) => {
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [errorMessage, setErrorMessage] = createSignal<string | null>(null);
  const [isSubmitting, setIsSubmitting] = createSignal(false);

  const handleSubmit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const organizationName = name().trim();

    if (organizationName.length === 0) {
      setErrorMessage("Enter an organization name.");

      return;
    }

    const organizationDescription = description().trim();

    setErrorMessage(null);
    setIsSubmitting(true);

    const result = await createOrganization(organizationDescription.length === 0
      ? { name: organizationName }
      : { name: organizationName, description: organizationDescription });

    setIsSubmitting(false);

    if (!result.ok) {
      setErrorMessage(result.message);

      return;
    }

    setName("");
    setDescription("");
    properties.onCreated(result.value);
  };

  return (
    <section class="border-t border-slate-200 pt-6 dark:border-slate-800">
      <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">New organization</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">An organization owns projects and carries its own member roles.</p>
      <form class="mt-4 space-y-3" onSubmit={event => void handleSubmit(event)}>
        <div class="space-y-1.5">
          <label for="new-organization-name" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
            Name
          </label>
          <input
            id="new-organization-name"
            type="text"
            value={name()}
            disabled={isSubmitting()}
            onInput={event => setName(event.currentTarget.value)}
            placeholder="Acme"
            class={FIELD}
          />
        </div>
        <div class="space-y-1.5">
          <label for="new-organization-description" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
            Description
          </label>
          <input
            id="new-organization-description"
            type="text"
            value={description()}
            disabled={isSubmitting()}
            onInput={event => setDescription(event.currentTarget.value)}
            placeholder="What this organization owns, in a sentence."
            class={FIELD}
          />
        </div>
        <Show when={errorMessage()}>
          {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
        </Show>
        <button
          type="submit"
          disabled={isSubmitting()}
          class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
        >
          {isSubmitting() ? "Creating..." : "Create organization"}
        </button>
      </form>
    </section>
  );
};

export const OrganizationsPage = () => {
  const account = useAccount();
  const [state, setState] = createSignal<OrganizationsState>({ phase: "loading" });
  let latestLoad = 0;

  const load = async (): Promise<void> => {
    const userId = account.user()?.user_id;

    if (userId === undefined) return;

    const loadId = ++latestLoad;
    const result = await listOrganizations();

    if (loadId !== latestLoad || account.user()?.user_id !== userId) return;

    if (!result.ok) {
      setState({ phase: "error", message: result.message });

      return;
    }

    setState({ phase: "loaded", organizations: result.value });
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

  const prependOrganization = (organization: Organization): void => {
    const current = state();

    if (current.phase !== "loaded") return;

    setState({ phase: "loaded", organizations: [organization, ...current.organizations] });
  };

  return (
    <div class="space-y-6">
      <h1 class="text-lg font-semibold">Organizations</h1>
      {renderOrganizations(state())}
      <Show when={account.user()}>
        {user => (
          <Show when={user().role !== "guest"}>
            <CreateOrganizationForm onCreated={prependOrganization} />
          </Show>
        )}
      </Show>
    </div>
  );
};
