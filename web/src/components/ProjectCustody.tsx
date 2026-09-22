import { useNavigate } from "@solidjs/router";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { Organization } from "../api/organizations";
import { listOrganizations } from "../api/organizations";
import type { Project, ProjectTransfer } from "../api/projects";
import { deleteProject, listProjectTransfers, moveProject } from "../api/projects";

type CustodyState = { phase: "ready"; } | { phase: "busy"; } | { phase: "error"; message: string; };
type OrganizationsState
  = | { phase: "loading"; }
    | { phase: "loaded"; organizations: readonly Organization[]; }
    | { phase: "error"; message: string; };
type TransfersState
  = | { phase: "loading"; }
    | { phase: "loaded"; transfers: readonly ProjectTransfer[]; }
    | { phase: "error"; message: string; };

const movedAtFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

export const ProjectCustody = (properties: { project: Project; onMoved: () => void; }) => {
  const navigate = useNavigate();
  const [organizationsState, setOrganizationsState] = createSignal<OrganizationsState>({ phase: "loading" });
  const [transfersState, setTransfersState] = createSignal<TransfersState>({ phase: "loading" });
  const [chosenOrganizationId, setChosenOrganizationId] = createSignal("");
  const [transferState, setTransferState] = createSignal<CustodyState>({ phase: "ready" });
  const [deleteState, setDeleteState] = createSignal<CustodyState>({ phase: "ready" });
  const [isArmed, setIsArmed] = createSignal(false);

  const load = async (projectId: string): Promise<void> => {
    const [organizations, transfers] = await Promise.all([
      listOrganizations(),
      listProjectTransfers(projectId),
    ]);

    setOrganizationsState(organizations.ok
      ? {
          phase: "loaded",
          organizations: organizations.value.filter(organization => organization.viewer_role === "owner"),
        }
      : { phase: "error", message: organizations.message });
    setTransfersState(transfers.ok
      ? { phase: "loaded", transfers: transfers.value }
      : { phase: "error", message: transfers.message });
  };

  createEffect(
    () => properties.project.project_id,
    (projectId) => {
      void load(projectId);
    },
  );

  // A project cannot move to the organization it is already in, and the server answers 400
  // if it is asked to.
  const destinations = (): readonly Organization[] => {
    const current = organizationsState();

    return current.phase === "loaded"
      ? current.organizations.filter(
          organization => organization.organization_id !== properties.project.organization_id,
        )
      : [];
  };

  const selectedOrganizationId = (): string | undefined => {
    const chosen = chosenOrganizationId();

    return chosen === "" ? destinations()[0]?.organization_id : chosen;
  };

  const transferError = (): string | undefined => {
    const current = transferState();

    if (current.phase === "error") return current.message;

    const organizations = organizationsState();

    return organizations.phase === "error" ? organizations.message : undefined;
  };

  const deleteError = (): string | undefined => {
    const current = deleteState();

    return current.phase === "error" ? current.message : undefined;
  };

  const transfers = (): readonly ProjectTransfer[] => {
    const current = transfersState();

    return current.phase === "loaded" ? current.transfers : [];
  };

  const transfer = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const organizationId = selectedOrganizationId();

    if (organizationId === undefined) return;

    setTransferState({ phase: "busy" });

    const result = await moveProject(properties.project.project_id, organizationId);

    if (!result.ok) {
      setTransferState({ phase: "error", message: result.message });

      return;
    }

    setTransferState({ phase: "ready" });
    setChosenOrganizationId("");
    void load(properties.project.project_id);
    properties.onMoved();
  };

  const remove = async (): Promise<void> => {
    setDeleteState({ phase: "busy" });

    const result = await deleteProject(properties.project.project_id);

    if (!result.ok) {
      setDeleteState({ phase: "error", message: result.message });
      setIsArmed(false);

      return;
    }

    navigate("/");
  };

  return (
    <section class="mt-6 border-t border-slate-200 pt-5 dark:border-slate-800">
      <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Organization</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
        {`This project belongs to ${properties.project.organization_name}. Moving it means everyone who reads it through ${properties.project.organization_name}, and who is not a member of the organization it goes to, loses it. A member added to this project by name keeps it. Both organizations need an owner grant from you.`}
      </p>
      <Show when={transferError()}>
        {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <Switch>
        <Match when={organizationsState().phase === "loading"}>
          <p class="mt-4 text-sm text-slate-500 dark:text-slate-400" role="status">Loading organizations...</p>
        </Match>
        <Match when={destinations().length === 0}>
          <p class="mt-4 text-sm text-slate-500 dark:text-slate-400">
            You own no other organization to move this project to.
          </p>
        </Match>
        <Match when={destinations().length > 0}>
          <form class="mt-4 flex flex-col gap-2 sm:flex-row" onSubmit={event => void transfer(event)}>
            <label class="min-w-0 flex-1">
              <span class="sr-only">Organization to move this project to</span>
              <select
                value={selectedOrganizationId() ?? ""}
                disabled={transferState().phase === "busy"}
                onChange={event => setChosenOrganizationId(event.currentTarget.value)}
                class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
              >
                <For each={destinations()}>
                  {organization => <option value={organization.organization_id}>{organization.name}</option>}
                </For>
              </select>
            </label>
            <button
              type="submit"
              disabled={transferState().phase === "busy"}
              class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
            >
              {transferState().phase === "busy" ? "Moving..." : "Move project"}
            </button>
          </form>
        </Match>
      </Switch>
      <Show when={transfers().length > 0}>
        <ul class="mt-4 space-y-1">
          <For each={transfers()}>
            {moved => (
              <li class="text-sm text-slate-500 dark:text-slate-400">
                {`Moved from ${moved.from_organization_name} to ${moved.to_organization_name} by ${moved.moved_by_name}, ${movedAtFormatter.format(new Date(moved.moved_at))}`}
              </li>
            )}
          </For>
        </ul>
      </Show>
      <h2 class="mt-6 text-base font-semibold text-slate-900 dark:text-slate-100">Delete</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
        Deleting this project removes its analyses, findings, issues, queue entries, grants and move history. Nothing here can be read back afterwards.
      </p>
      <Show when={deleteError()}>
        {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <div class="mt-4 flex items-center gap-2">
        <button
          type="button"
          disabled={deleteState().phase === "busy"}
          onClick={() => (isArmed() ? void remove() : setIsArmed(true))}
          class="rounded-md border border-red-600 px-3 py-1.5 text-sm font-medium text-red-600 hover:bg-red-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-red-400 dark:text-red-400 dark:hover:bg-red-950"
        >
          <Switch fallback="Delete project">
            <Match when={deleteState().phase === "busy"}>Deleting...</Match>
            <Match when={isArmed()}>{`Delete ${properties.project.name} for good`}</Match>
          </Switch>
        </button>
        <Show when={isArmed() && deleteState().phase !== "busy"}>
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
