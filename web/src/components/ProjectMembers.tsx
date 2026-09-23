import { Popover } from "@kobalte/core/popover";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { ProjectMember, ProjectRole } from "../api/projects";
import { listProjectMembers, removeProjectMember, setProjectMember } from "../api/projects";
import type { User } from "../api/users";
import { listUsers } from "../api/users";

type MembersState
  = | { phase: "loading"; }
    | { phase: "loaded"; members: readonly ProjectMember[]; }
    | { phase: "error"; message: string; };
type UsersState
  = | { phase: "loading"; }
    | { phase: "loaded"; users: readonly User[]; }
    | { phase: "error"; message: string; };
type MemberWriteState = { phase: "ready"; } | { phase: "busy"; userId: string; } | { phase: "error"; message: string; };

const PROJECT_ROLES: readonly ProjectRole[] = ["viewer", "operator", "owner"];

export const ProjectMembers = (properties: { projectId: string; }) => {
  const [membersState, setMembersState] = createSignal<MembersState>({ phase: "loading" });
  const [usersState, setUsersState] = createSignal<UsersState>({ phase: "loading" });
  const [query, setQuery] = createSignal("");
  const [writeState, setWriteState] = createSignal<MemberWriteState>({ phase: "ready" });

  const load = async (projectId: string): Promise<void> => {
    setMembersState({ phase: "loading" });
    setUsersState({ phase: "loading" });
    setWriteState({ phase: "ready" });
    setQuery("");

    const [members, users] = await Promise.all([listProjectMembers(projectId), listUsers()]);

    setMembersState(members.ok
      ? { phase: "loaded", members: members.value }
      : { phase: "error", message: members.message });
    setUsersState(users.ok
      ? { phase: "loaded", users: users.value }
      : { phase: "error", message: users.message });
  };

  createEffect(
    () => properties.projectId,
    (projectId) => {
      void load(projectId);
    },
  );

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

  const addMember = async (userId: string): Promise<void> => {
    if (await saveMember(userId, "viewer")) setQuery("");
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

  const matches = (): readonly User[] => {
    const needle = query()
      .trim()
      .toLowerCase();

    return needle === ""
      ? candidates()
      : candidates().filter(candidate => candidate.display_name.toLowerCase().includes(needle));
  };

  return (
    <section class="space-y-4">
      <Show when={writeError()}>
        {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <Show when={membersState().phase === "loading"}>
        <p class="text-sm text-slate-500 dark:text-slate-400" role="status">Loading members...</p>
      </Show>
      <Show when={membersError()}>
        {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <Show when={members()}>
        {loadedMembers => (
          <ul class="divide-y divide-hairline">
            <For each={loadedMembers()}>
              {member => (
                <li class="flex items-center justify-between gap-4 px-1 py-3">
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
                        class="rounded-control bg-raised px-2 py-1 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-100"
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
                      class="rounded-control bg-raised px-2 py-1 text-sm font-medium text-slate-700 hover:bg-raised-hover disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-300"
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
      <Popover placement="bottom-start" gutter={6} onOpenChange={() => setQuery("")}>
        <Popover.Trigger class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-300">
          Add member
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content class="z-50 w-72 rounded-panel bg-surface p-2 shadow-lg">
            <input
              type="search"
              value={query()}
              aria-label="Search users"
              placeholder="Search users"
              onInput={event => setQuery(event.currentTarget.value)}
              class="w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100"
            />
            <Switch>
              <Match when={usersState().phase === "loading"}>
                <p class="px-3 py-4 text-sm text-slate-500 dark:text-slate-400" role="status">Loading users...</p>
              </Match>
              <Match when={usersError()}>
                {message => <p class="px-3 py-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
              </Match>
              <Match when={candidates().length === 0}>
                <p class="px-3 py-4 text-sm text-slate-500 dark:text-slate-400">Every eligible user is already a member.</p>
              </Match>
              <Match when={candidates().length > 0}>
                <ul class="mt-2 max-h-56 overflow-y-auto">
                  <For
                    each={matches()}
                    fallback={<li class="px-3 py-4 text-sm text-slate-500 dark:text-slate-400">No user matches that search.</li>}
                  >
                    {candidate => (
                      <li>
                        <button
                          type="button"
                          disabled={writeState().phase === "busy"}
                          onClick={() => void addMember(candidate.user_id)}
                          class="w-full truncate rounded-control px-3 py-2 text-left text-sm text-slate-700 hover:bg-raised disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-200"
                        >
                          {candidate.display_name}
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </Match>
            </Switch>
          </Popover.Content>
        </Popover.Portal>
      </Popover>
      <Show when={members()?.length === 0}>
        <p class="text-sm text-slate-500 dark:text-slate-400">No member yet.</p>
      </Show>
    </section>
  );
};
