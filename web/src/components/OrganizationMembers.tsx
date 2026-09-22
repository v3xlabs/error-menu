import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { OrganizationMember, OrganizationRole } from "../api/organizations";
import { listOrganizationMembers, removeOrganizationMember, setOrganizationMember } from "../api/organizations";
import type { User } from "../api/users";
import { listUsers } from "../api/users";

type MembersState
  = | { phase: "loading"; }
    | { phase: "loaded"; members: readonly OrganizationMember[]; }
    | { phase: "error"; message: string; };
type UsersState
  = | { phase: "loading"; }
    | { phase: "loaded"; users: readonly User[]; }
    | { phase: "error"; message: string; };
type MemberWriteState = { phase: "ready"; } | { phase: "busy"; userId: string; } | { phase: "error"; message: string; };

const ORGANIZATION_ROLES: readonly OrganizationRole[] = ["viewer", "operator", "owner"];

export const OrganizationMembers = (properties: { organizationId: string; }) => {
  const [membersState, setMembersState] = createSignal<MembersState>({ phase: "loading" });
  const [usersState, setUsersState] = createSignal<UsersState>({ phase: "loading" });
  const [newUserId, setNewUserId] = createSignal("");
  const [newRole, setNewRole] = createSignal<OrganizationRole>("viewer");
  const [writeState, setWriteState] = createSignal<MemberWriteState>({ phase: "ready" });

  const load = async (organizationId: string): Promise<void> => {
    setMembersState({ phase: "loading" });
    setUsersState({ phase: "loading" });
    setWriteState({ phase: "ready" });
    setNewUserId("");

    const [members, users] = await Promise.all([listOrganizationMembers(organizationId), listUsers()]);

    setMembersState(members.ok
      ? { phase: "loaded", members: members.value }
      : { phase: "error", message: members.message });
    setUsersState(users.ok
      ? { phase: "loaded", users: users.value }
      : { phase: "error", message: users.message });
  };

  createEffect(
    () => properties.organizationId,
    (organizationId) => {
      void load(organizationId);
    },
  );

  const saveMember = async (userId: string, role: OrganizationRole): Promise<boolean> => {
    setWriteState({ phase: "busy", userId });

    const result = await setOrganizationMember(properties.organizationId, userId, role);

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

    const result = await removeOrganizationMember(properties.organizationId, userId);

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return;
    }

    setMembersState({ phase: "loaded", members: result.value });
    setWriteState({ phase: "ready" });
  };

  const changeRole = (userId: string, roleValue: string): void => {
    const role = ORGANIZATION_ROLES.find(candidate => candidate === roleValue);

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

  const members = (): readonly OrganizationMember[] | undefined => {
    const current = membersState();

    return current.phase === "loaded" ? current.members : undefined;
  };
  // An organization grant works on a guest, so every user that is not a member yet belongs
  // in the picker.
  const candidates = (): readonly User[] => {
    const users = usersState();
    const current = membersState();

    if (users.phase !== "loaded" || current.phase !== "loaded") return [];

    return users.users.filter(user => current.members.every(member => member.user_id !== user.user_id));
  };

  return (
    <section class="mt-6 border-t border-slate-200 pt-5 dark:border-slate-800">
      <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Members</h2>
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
                    <span id={`organization-member-${member.user_id}`} class="sr-only">{` (identifier ${member.user_id})`}</span>
                  </p>
                  <div class="flex shrink-0 items-center gap-2">
                    <label aria-label={`Role for ${member.display_name}`} class="text-sm text-slate-700 dark:text-slate-300">
                      <select
                        value={member.role}
                        disabled={isBusy(member.user_id)}
                        aria-describedby={`organization-member-${member.user_id}`}
                        onChange={event => changeRole(member.user_id, event.currentTarget.value)}
                        class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                      >
                        <For each={ORGANIZATION_ROLES}>{role => <option value={role}>{role}</option>}</For>
                      </select>
                    </label>
                    <button
                      type="button"
                      disabled={isBusy(member.user_id)}
                      aria-label={`Remove ${member.display_name}`}
                      aria-describedby={`organization-member-${member.user_id}`}
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
          <p class="mt-4 text-sm text-slate-500 dark:text-slate-400">Every user is already a member.</p>
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
                  const role = ORGANIZATION_ROLES.find(candidate => candidate === roleValue);

                  if (role !== undefined) setNewRole(role);
                }}
                class="w-full rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm text-slate-900 sm:w-auto dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
              >
                <For each={ORGANIZATION_ROLES}>{role => <option value={role}>{role}</option>}</For>
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
    </section>
  );
};
