import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Organization } from "../api/organizations";
import { listOrganizations } from "../api/organizations";
import { useAccount } from "../app/account";
import { AddOrganizationModal } from "../components/AddOrganizationModal";

type OrganizationsState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; organizations: readonly Organization[]; }
    | { phase: "error"; message: string; };

const renderOrganizations = (state: OrganizationsState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Loading organizations...</p>;
    }
    case "anonymous": {
      return <p class="rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500">Sign in to view organizations.</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400" role="alert">{state.message}</p>;
    }
    case "loaded": {
      return (
        <Show when={state.organizations.length > 0} fallback={<p class="rounded-panel bg-surface px-4 py-8 text-center text-sm text-slate-500 dark:text-slate-500">No organizations yet.</p>}>
          <ul class="divide-y divide-hairline overflow-hidden rounded-panel bg-surface">
            <For each={state.organizations}>
              {organization => (
                <li>
                  <a href={`/orgs/${organization.organization_id}`} class="flex items-center justify-between gap-4 px-5 py-3.5 hover:bg-raised">
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
      <div class="flex items-center justify-between">
        <h1 class="text-lg font-semibold">Organizations</h1>
        <Show when={account.user()}>
          {user => (
            <Show when={user().role !== "guest"}>
              <AddOrganizationModal onCreated={prependOrganization} />
            </Show>
          )}
        </Show>
      </div>
      {renderOrganizations(state())}
    </div>
  );
};
