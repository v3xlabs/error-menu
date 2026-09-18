import { Dialog } from "@kobalte/core/dialog";
import { createSignal, For, Match, Show, Switch } from "solid-js";

import type { ProjectMember, ProjectRole } from "../api/projects";
import { listProjectMembers, removeProjectMember, setProjectMember } from "../api/projects";
import type { User } from "../api/users";
import { listUsers } from "../api/users";

type MembersState
  = | { phase: "idle"; }
    | { phase: "loading"; }
    | { phase: "loaded"; members: readonly ProjectMember[]; }
    | { phase: "error"; message: string; };
type UsersState
  = | { phase: "idle"; }
    | { phase: "loading"; }
    | { phase: "loaded"; users: readonly User[]; }
    | { phase: "error"; message: string; };
type WriteState = { phase: "ready"; } | { phase: "busy"; userId: string; } | { phase: "error"; message: string; };

const PROJECT_ROLES: readonly ProjectRole[] = ["viewer", "operator", "owner"];

export const ProjectMembersDialog = (properties: { projectId: string; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [membersState, setMembersState] = createSignal<MembersState>({ phase: "idle" });
  const [usersState, setUsersState] = createSignal<UsersState>({ phase: "idle" });
  const [newUserId, setNewUserId] = createSignal("");
  const [newRole, setNewRole] = createSignal<ProjectRole>("viewer");
  const [writeState, setWriteState] = createSignal<WriteState>({ phase: "ready" });

  const load = async (): Promise<void> => {
    setMembersState({ phase: "loading" });
    setUsersState({ phase: "loading" });
    setWriteState({ phase: "ready" });
    setNewUserId("");

    const [members, users] = await Promise.all([listProjectMembers(properties.projectId), listUsers()]);

    setMembersState(members.ok
      ? { phase: "loaded", members: members.value }
      : { phase: "error", message: members.message });
    setUsersState(users.ok
      ? { phase: "loaded", users: users.value }
      : { phase: "error", message: users.message });
  };

  const handleOpenChange = (isNowOpen: boolean): void => {
    setIsOpen(isNowOpen);

    if (isNowOpen) void load();
  };

  const saveMember = async (userId: string, role: ProjectRole): Promise<boolean> => {
    setWriteState({ phase: "busy", userId });

    const result = await setProjectMember(properties.projectId, userId, role);

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return false;
    }

    setMembersState({ phase: "loaded", members: result.value });
    setWriteState({ phase: "ready" });

    return true;
  };

  const dropMember = async (userId: string): Promise<void> => {
    setWriteState({ phase: "busy", userId });

    const result = await removeProjectMember(properties.projectId, userId);

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return;
    }

    setMembersState({ phase: "loaded", members: result.value });
    setWriteState({ phase: "ready" });
  };

  const changeRole = (userId: string, roleValue: string): void => {
    const role = PROJECT_ROLES.find(candidate => candidate === roleValue);

    if (role !== undefined) void saveMember(userId, role);
  };

  const addMember = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const userId = newUserId();

    if (userId.length === 0) {
      setWriteState({ phase: "error", message: "Select a user to add." });

      return;
    }

    if (await saveMember(userId, newRole())) setNewUserId("");
  };

  const writeError = (): string | undefined => {
    const current = writeState();

    return current.phase === "error" ? current.message : undefined;
  };

  const membersError = (): string | undefined => {
    const current = membersState();

    return current.phase === "error" ? current.message : undefined;
  };

  const usersError = (): string | undefined => {
    const current = usersState();

    return current.phase === "error" ? current.message : undefined;
  };

  const isBusy = (userId: string): boolean => {
    const current = writeState();

    return current.phase === "busy" && current.userId === userId;
  };

  const members = (): readonly ProjectMember[] | undefined => {
    const current = membersState();

    return current.phase === "loaded" ? current.members : undefined;
  };
  // A guest cannot hold a project role, and an existing member has one already, so
  // neither belongs in the picker.
  const candidates = (): readonly User[] => {
    const users = usersState();
    const current = membersState();

    if (users.phase !== "loaded" || current.phase !== "loaded") return [];

    return users.users.filter(
      user => user.role !== "guest" && current.members.every(member => member.user_id !== user.user_id),
    );
  };

  return (
    <Dialog open={isOpen()} onOpenChange={handleOpenChange}>
      <Dialog.Trigger class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-100 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800">
        Members
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-lg border border-slate-200 bg-white p-5 shadow-xl dark:border-slate-800 dark:bg-slate-900">
            <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Project members</Dialog.Title>
            <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">Set a member role, remove a member, or add another user.</p>
            <Show when={writeError()}>
              {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
            </Show>
            <Show when={membersState().phase === "loading"}>
              <p class="mt-4 text-sm text-slate-500 dark:text-slate-400" role="status">Loading members...</p>
            </Show>
            <Show when={membersError()}>
              {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
            </Show>
            <Show when={members()}>
              {loadedMembers => (
                <ul class="mt-4 divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
                  <For
                    each={loadedMembers()}
                    fallback={<li class="px-3 py-6 text-center text-sm text-slate-500 dark:text-slate-400">No member yet.</li>}
                  >
                    {member => (
                      <li class="flex items-center justify-between gap-4 px-3 py-2.5">
                        <p class="min-w-0 truncate text-sm font-medium text-slate-900 dark:text-slate-100">
                          {member.display_name}
                          <span id={`project-member-${member.user_id}`} class="sr-only">{` (identifier ${member.user_id})`}</span>
                        </p>
                        <div class="flex shrink-0 items-center gap-2">
                          <label aria-label={`Role for ${member.display_name}`} class="text-sm text-slate-700 dark:text-slate-300">
                            <select
                              value={member.role}
                              disabled={isBusy(member.user_id)}
                              aria-describedby={`project-member-${member.user_id}`}
                              onChange={event => changeRole(member.user_id, event.currentTarget.value)}
                              class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                            >
                              <For each={PROJECT_ROLES}>{role => <option value={role}>{role}</option>}</For>
                            </select>
                          </label>
                          <button
                            type="button"
                            disabled={isBusy(member.user_id)}
                            aria-label={`Remove ${member.display_name}`}
                            aria-describedby={`project-member-${member.user_id}`}
                            onClick={() => void dropMember(member.user_id)}
                            class="rounded-md border border-slate-300 px-2 py-1 text-sm font-medium text-slate-700 hover:bg-slate-100 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
                          >
                            {isBusy(member.user_id) ? "Removing..." : "Remove"}
                          </button>
                        </div>
                      </li>
                    )}
                  </For>
                </ul>
              )}
            </Show>
            <Switch>
              <Match when={usersState().phase === "loading"}>
                <p class="mt-4 text-sm text-slate-500 dark:text-slate-400" role="status">Loading users...</p>
              </Match>
              <Match when={usersError()}>
                {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
              </Match>
              <Match when={candidates().length === 0}>
                <p class="mt-4 text-sm text-slate-500 dark:text-slate-400">Every eligible user is already a member.</p>
              </Match>
              <Match when={candidates().length > 0}>
                <form class="mt-4 flex flex-col gap-2 sm:flex-row" onSubmit={event => void addMember(event)}>
                  <label class="min-w-0 flex-1">
                    <span class="sr-only">User to add</span>
                    <select
                      value={newUserId()}
                      onChange={event => setNewUserId(event.currentTarget.value)}
                      class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                    >
                      <option value="">Select a user</option>
                      <For each={candidates()}>
                        {candidate => <option value={candidate.user_id}>{candidate.display_name}</option>}
                      </For>
                    </select>
                  </label>
                  <label>
                    <span class="sr-only">New member role</span>
                    <select
                      value={newRole()}
                      onChange={(event) => {
                        const roleValue = event.currentTarget.value;
                        const role = PROJECT_ROLES.find(candidate => candidate === roleValue);

                        if (role !== undefined) setNewRole(role);
                      }}
                      class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 sm:w-auto dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                    >
                      <For each={PROJECT_ROLES}>{role => <option value={role}>{role}</option>}</For>
                    </select>
                  </label>
                  <button
                    type="submit"
                    disabled={writeState().phase === "busy"}
                    class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
                  >
                    Add member
                  </button>
                </form>
              </Match>
            </Switch>
            <div class="mt-5 flex justify-end">
              <Dialog.CloseButton class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800">
                Close
              </Dialog.CloseButton>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
