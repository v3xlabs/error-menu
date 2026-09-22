import { Dialog } from "@kobalte/core/dialog";
import { createSignal, Show } from "solid-js";

import type { Organization } from "../api/organizations";
import { createOrganization } from "../api/organizations";

const FIELD = "w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-100";

export const AddOrganizationModal = (properties: { onCreated: (organization: Organization) => void; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [errorMessage, setErrorMessage] = createSignal<string | null>(null);
  const [isSubmitting, setIsSubmitting] = createSignal(false);

  const handleOpenChange = (isNowOpen: boolean): void => {
    setIsOpen(isNowOpen);

    if (isNowOpen) return;

    setName("");
    setDescription("");
    setErrorMessage(null);
  };

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

    handleOpenChange(false);
    properties.onCreated(result.value);
  };

  return (
    <Dialog open={isOpen()} onOpenChange={handleOpenChange}>
      <Dialog.Trigger class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
        New organization
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="w-full max-w-md rounded-panel bg-surface p-5 shadow-xl">
            <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">New organization</Dialog.Title>
            <Dialog.Description class="mt-1 text-sm text-slate-500 dark:text-slate-400">
              An organization owns projects and carries its own member roles.
            </Dialog.Description>
            <form class="mt-4 space-y-4" onSubmit={event => void handleSubmit(event)}>
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
              <div class="flex justify-end gap-2 pt-1">
                <Dialog.CloseButton class="rounded-control bg-raised px-3 py-1.5 text-sm text-slate-700 hover:bg-raised-hover dark:text-slate-300">
                  Cancel
                </Dialog.CloseButton>
                <button
                  type="submit"
                  disabled={isSubmitting()}
                  class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
                >
                  {isSubmitting() ? "Creating..." : "Create organization"}
                </button>
              </div>
            </form>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
