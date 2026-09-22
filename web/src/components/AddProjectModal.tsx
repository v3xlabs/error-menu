import { Dialog } from "@kobalte/core/dialog";
import { createEffect, createSignal, For, Match, Switch } from "solid-js";

import type { Organization } from "../api/organizations";
import { listOrganizations } from "../api/organizations";
import { createProject } from "../api/projects";
import type { AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { isValidRemoteUrl } from "../domain/project";
import { AnalyzerSelect } from "./AnalyzerSelect";
import { InfoTip } from "./InfoTip";

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
    setUsesDefaultAnalyzers(true);
    setEnabledAnalyzers([]);
    setErrorMessage(null);
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
      <Dialog.Trigger class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
        Add project
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="w-full max-w-md rounded-lg border border-slate-200 bg-white p-5 shadow-xl dark:border-slate-800 dark:bg-slate-900">
            <div class="flex items-center gap-1.5">
              <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Add project</Dialog.Title>
              <InfoTip
                label="What a project is"
                text="A project is one watched repository. It holds the remote, the forge link error.menu infers from it, and which analyzers run over its changes."
              />
            </div>
            <form class="mt-4 space-y-4" onSubmit={event => void handleSubmit(event)}>
              <Switch>
                <Match when={organizations().phase === "loading"}>
                  <p class="text-sm text-slate-500 dark:text-slate-500">Loading organizations...</p>
                </Match>
                <Match when={organizationsErrorMessage()}>
                  {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
                </Match>
                <Match when={ownedOrganizations().length === 0}>
                  <p class="text-sm text-slate-600 dark:text-slate-300">
                    A project belongs to an organization you own.
                    {" "}
                    <a href="/orgs" class="font-medium text-slate-900 underline dark:text-slate-100">Create an organization</a>
                    {" first."}
                  </p>
                </Match>
                <Match when={ownedOrganizations().length > 0}>
                  <div class="space-y-1.5">
                    <label for="organization" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
                      Organization
                    </label>
                    <select
                      id="organization"
                      required
                      value={selectedOrganizationId() ?? ""}
                      onChange={event => setChosenOrganizationId(event.currentTarget.value)}
                      class="w-full rounded-md border border-slate-300 bg-white px-3 py-1.5 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
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
                      onInput={event => setRemoteUrl(event.currentTarget.value)}
                      placeholder="https://github.com/acme/example"
                      class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
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
                      onInput={event => setDisplayName(event.currentTarget.value)}
                      placeholder="example"
                      class="w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                    />
                  </div>
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
                </Match>
              </Switch>
              {errorMessage() !== null && <p class="text-sm text-red-600 dark:text-red-400">{errorMessage()}</p>}
              <div class="flex justify-end gap-2 pt-1">
                <Dialog.CloseButton class="rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-700 dark:border-slate-700 dark:text-slate-300">
                  Cancel
                </Dialog.CloseButton>
                <button type="submit" disabled={isSubmitting() || ownedOrganizations().length === 0} class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
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
