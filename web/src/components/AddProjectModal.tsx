import { Dialog } from "@kobalte/core/dialog";
import { Tabs } from "@kobalte/core/tabs";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { Organization } from "../api/organizations";
import { listOrganizations } from "../api/organizations";
import { createProject } from "../api/projects";
import type { AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { isValidRemoteUrl, repoName } from "../domain/project";
import { AnalyzerSelect } from "./AnalyzerSelect";
import { InfoTip } from "./InfoTip";
import { TAB_LIST, TAB_TRIGGER } from "./Tabs";

const DEFAULT_ANALYZERS: readonly AnalyzerId[] = ANALYZERS.map(analyzer => analyzer.analyzerId);

type OrganizationsState
  = | { phase: "loading"; }
    | { phase: "loaded"; organizations: readonly Organization[]; }
    | { phase: "error"; message: string; };

export const AddProjectModal = (properties: { onCreated: () => void; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [organizations, setOrganizations] = createSignal<OrganizationsState>({ phase: "loading" });
  const [chosenOrganizationId, setChosenOrganizationId] = createSignal<string | null>(null);
  const [remoteUrl, setRemoteUrl] = createSignal("");
  const [displayName, setDisplayName] = createSignal("");
  const [isNameTyped, setIsNameTyped] = createSignal(false);
  const [usesDefaultAnalyzers, setUsesDefaultAnalyzers] = createSignal(true);
  const [enabledAnalyzers, setEnabledAnalyzers] = createSignal<readonly AnalyzerId[]>([]);
  const [errorMessage, setErrorMessage] = createSignal<string | null>(null);
  const [isSubmitting, setIsSubmitting] = createSignal(false);
  let latestOrganizationsLoad = 0;

  const ownedOrganizations = (): readonly Organization[] => {
    const current = organizations();

    return current.phase === "loaded" ? current.organizations : [];
  };

  const organizationsErrorMessage = (): string | undefined => {
    const current = organizations();

    return current.phase === "error" ? current.message : undefined;
  };

  const selectedOrganizationId = (): string | undefined => {
    const chosen = chosenOrganizationId();

    if (chosen !== null && ownedOrganizations().some(organization => organization.organization_id === chosen)) {
      return chosen;
    }

    return ownedOrganizations().at(0)?.organization_id;
  };

  const loadOrganizations = async (): Promise<void> => {
    const loadId = ++latestOrganizationsLoad;

    setOrganizations({ phase: "loading" });

    const result = await listOrganizations();

    if (loadId !== latestOrganizationsLoad) return;

    if (!result.ok) {
      setOrganizations({ phase: "error", message: result.message });

      return;
    }

    setOrganizations({
      phase: "loaded",
      organizations: result.value.filter(organization => organization.viewer_role === "owner"),
    });
  };

  createEffect(
    () => isOpen(),
    (isNowOpen) => {
      if (isNowOpen) void loadOrganizations();
    },
  );

  const selectedAnalyzers = (): readonly AnalyzerId[] =>
    (usesDefaultAnalyzers() ? DEFAULT_ANALYZERS : enabledAnalyzers());

  const toggleSelectedAnalyzer = (analyzerId: AnalyzerId): void => {
    const current = selectedAnalyzers();

    setUsesDefaultAnalyzers(false);
    setEnabledAnalyzers(
      current.includes(analyzerId) ? current.filter(analyzer => analyzer !== analyzerId) : [...current, analyzerId],
    );
  };

  const resetForm = (): void => {
    setChosenOrganizationId(null);
    setRemoteUrl("");
    setDisplayName("");
    setIsNameTyped(false);
    setUsesDefaultAnalyzers(true);
    setEnabledAnalyzers([]);
    setErrorMessage(null);
  };

  const changeRemoteUrl = (next: string): void => {
    setRemoteUrl(next);

    if (!isNameTyped()) setDisplayName(repoName(next));
  };

  // Clearing the name hands it back to the remote URL.
  const changeDisplayName = (next: string): void => {
    setDisplayName(next);
    setIsNameTyped(next.length > 0);
  };

  const handleOpenChange = (isNowOpen: boolean): void => {
    setIsOpen(isNowOpen);

    if (!isNowOpen) resetForm();
  };

  const handleSubmit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const organizationId = selectedOrganizationId();

    if (organizationId === undefined) {
      setErrorMessage("Create an organization before you add a project.");

      return;
    }

    if (!isValidRemoteUrl(remoteUrl())) {
      setErrorMessage("Enter a remote URL that starts with https:// or ssh://.");

      return;
    }

    if (displayName().trim().length === 0) {
      setErrorMessage("Enter a project name.");

      return;
    }

    setIsSubmitting(true);
    const result = await createProject({
      organization_id: organizationId,
      name: displayName().trim(),
      remote_url: remoteUrl(),
      uses_default_analyzers: usesDefaultAnalyzers(),
      analyzers: [...selectedAnalyzers()],
    });

    setIsSubmitting(false);

    if (!result.ok) {
      setErrorMessage(result.message);

      return;
    }

    handleOpenChange(false);
    properties.onCreated();
  };

  return (
    <Dialog open={isOpen()} onOpenChange={handleOpenChange}>
      <Dialog.Trigger class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
        Add project
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="flex h-128 max-h-[85vh] w-full max-w-lg flex-col rounded-panel bg-surface p-5 shadow-xl">
            <div class="flex items-center gap-1.5">
              <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Add project</Dialog.Title>
              <InfoTip
                label="What a project is"
                text="A project is one watched repository. It holds the remote, the forge link error.menu infers from it, and which analyzers run over its changes."
              />
            </div>
            <form class="mt-4 flex min-h-0 flex-1 flex-col" onSubmit={event => void handleSubmit(event)}>
              <Switch>
                <Match when={organizations().phase === "loading"}>
                  <p class="flex-1 text-sm text-slate-500 dark:text-slate-500">Loading organizations...</p>
                </Match>
                <Match when={organizationsErrorMessage()}>
                  {message => <p class="flex-1 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
                </Match>
                <Match when={ownedOrganizations().length === 0}>
                  <p class="flex-1 text-sm text-slate-600 dark:text-slate-300">
                    A project belongs to an organization you own.
                    {" "}
                    <a href="/orgs" class="font-medium text-slate-900 underline dark:text-slate-100">Create an organization</a>
                    {" first."}
                  </p>
                </Match>
                <Match when={ownedOrganizations().length > 0}>
                  <Tabs class="flex min-h-0 flex-1 flex-col">
                    <Tabs.List class={TAB_LIST}>
                      <Tabs.Trigger value="info" class={TAB_TRIGGER}>Info</Tabs.Trigger>
                      <Tabs.Trigger value="analyzers" class={TAB_TRIGGER}>Analyzers</Tabs.Trigger>
                    </Tabs.List>

                    <Tabs.Content value="info" class="min-h-0 flex-1 space-y-4 overflow-y-auto pt-4">
                      <div class="space-y-1.5">
                        <label for="organization" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
                          Organization
                        </label>
                        <select
                          id="organization"
                          required
                          value={selectedOrganizationId() ?? ""}
                          onChange={event => setChosenOrganizationId(event.currentTarget.value)}
                          class="w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100"
                        >
                          <For each={ownedOrganizations()}>
                            {organization => <option value={organization.organization_id}>{organization.name}</option>}
                          </For>
                        </select>
                      </div>
                      <div class="space-y-1.5">
                        <label for="remote-url" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
                          Remote URL
                        </label>
                        <input
                          id="remote-url"
                          type="text"
                          value={remoteUrl()}
                          onInput={event => changeRemoteUrl(event.currentTarget.value)}
                          placeholder="https://github.com/acme/example"
                          class="w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100"
                        />
                      </div>
                      <div class="space-y-1.5">
                        <label for="display-name" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
                          Display name
                        </label>
                        <input
                          id="display-name"
                          type="text"
                          value={displayName()}
                          onInput={event => changeDisplayName(event.currentTarget.value)}
                          aria-describedby="display-name-source"
                          placeholder="example"
                          class="w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100"
                        />
                        <Show when={!isNameTyped() && displayName().length > 0}>
                          <p id="display-name-source" class="text-xs text-slate-500 dark:text-slate-500">Filled from the remote URL</p>
                        </Show>
                      </div>
                    </Tabs.Content>

                    <Tabs.Content value="analyzers" class="min-h-0 flex-1 overflow-y-auto pt-4">
                      <fieldset class="space-y-3">
                        <legend class="text-sm font-medium text-slate-700 dark:text-slate-300">Analyzer configuration</legend>
                        <div class="space-y-1.5">
                          <label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
                            <input
                              type="radio"
                              name="add-project-analyzer-mode"
                              checked={usesDefaultAnalyzers()}
                              onInput={() => setUsesDefaultAnalyzers(true)}
                            />
                            <span class="flex items-center gap-1.5 font-medium">
                              Use system defaults
                              <InfoTip label="What the defaults run" text="Runs every analyzer error.menu ships, including ones added later." />
                            </span>
                          </label>
                          <label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
                            <input
                              type="radio"
                              name="add-project-analyzer-mode"
                              checked={!usesDefaultAnalyzers()}
                              onInput={() => setUsesDefaultAnalyzers(false)}
                            />
                            <span class="font-medium">Choose analyzers</span>
                          </label>
                        </div>
                        <div class="space-y-1.5">
                          <p class="text-sm font-medium text-slate-700 dark:text-slate-300">Enabled analyzers</p>
                          <AnalyzerSelect selected={selectedAnalyzers()} onToggle={toggleSelectedAnalyzer} />
                        </div>
                      </fieldset>
                    </Tabs.Content>
                  </Tabs>
                </Match>
              </Switch>
              {errorMessage() !== null && <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{errorMessage()}</p>}
              <div class="mt-5 flex justify-end gap-2 border-t border-hairline pt-4">
                <Dialog.CloseButton class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300">
                  Cancel
                </Dialog.CloseButton>
                <button type="submit" disabled={isSubmitting() || ownedOrganizations().length === 0} class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
                  {isSubmitting() ? "Adding..." : "Add project"}
                </button>
              </div>
            </form>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
