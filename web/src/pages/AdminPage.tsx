import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { User, UserRole } from "../api/users";
import { listUsers, setUserRole } from "../api/users";
import { useAccount } from "../app/account";

type UsersState = { phase: "loading"; } | { phase: "loaded"; users: readonly User[]; } | { phase: "error"; message: string; };

const USER_ROLES: readonly UserRole[] = ["guest", "member", "admin"];

const UsersSection = (properties: { onChanged: () => void; }) => {
  const [state, setState] = createSignal<UsersState>({ phase: "loading" });
  const [savingUserId, setSavingUserId] = createSignal<string | null>(null);
  const [errorMessage, setErrorMessage] = createSignal<string | null>(null);

  const load = async (): Promise<void> => {
    setState({ phase: "loading" });
    setErrorMessage(null);

    const result = await listUsers();

    setState(result.ok ? { phase: "loaded", users: result.value } : { phase: "error", message: result.message });
  };

  createEffect(
    () => undefined,
    () => {
      void load();
    },
  );

  const updateRole = async (userId: string, role: UserRole): Promise<void> => {
    setSavingUserId(userId);
    setErrorMessage(null);

    const result = await setUserRole(userId, role);

    setSavingUserId(null);

    if (!result.ok) {
      setErrorMessage(result.message);

      return;
    }

    const current = state();

    if (current.phase === "loaded") {
      setState({
        phase: "loaded",
        users: current.users.map((user) => {
          if (user.user_id === userId) return result.value;

          return user;
        }),
      });
    }

    properties.onChanged();
  };

  const handleRoleChange = (userId: string, roleValue: string): void => {
    const role = USER_ROLES.find(candidate => candidate === roleValue);

    if (role !== undefined) void updateRole(userId, role);
  };

  const loadError = (): string | null => {
    const current = state();

    return current.phase === "error" ? current.message : null;
  };

  const users = (): readonly User[] | undefined => {
    const current = state();

    return current.phase === "loaded" ? current.users : undefined;
  };

  return (
    <section>
      <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Users</h2>
      <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">Set the application role for a signed-in user.</p>
      <Show when={errorMessage()}>{message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}</Show>
      <Show when={state().phase === "loading"}>
        <p class="mt-4 text-sm text-slate-500 dark:text-slate-400" role="status">Loading users...</p>
      </Show>
      <Show when={loadError()}>{message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}</Show>
      <Show when={users()}>
        {loadedUsers => (
          <ul class="mt-4 divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For
              each={loadedUsers()}
              fallback={<li class="px-3 py-6 text-center text-sm text-slate-500 dark:text-slate-400">No signed-in users yet.</li>}
            >
              {user => (
                <li class="flex items-center justify-between gap-4 px-3 py-2.5">
                  <div class="min-w-0">
                    <p class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{user.display_name}</p>
                    <p class="truncate font-mono text-xs text-slate-500 dark:text-slate-400">{user.user_id}</p>
                  </div>
                  <label aria-label={`Role for ${user.display_name}`} class="shrink-0 text-sm text-slate-700 dark:text-slate-300">
                    <select
                      value={user.role}
                      disabled={savingUserId() === user.user_id}
                      onChange={event => handleRoleChange(user.user_id, event.currentTarget.value)}
                      class="rounded-md border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
                    >
                      <For each={USER_ROLES}>{role => <option value={role}>{role}</option>}</For>
                    </select>
                  </label>
                </li>
              )}
            </For>
          </ul>
        )}
      </Show>
    </section>
  );
};

export const AdminPage = () => {
  const account = useAccount();

  return (
    <div class="space-y-8">
      <h1 class="text-lg font-semibold">Admin</h1>
      <Switch>
        <Match when={account.state().phase === "loading"}>
          <p class="text-sm text-slate-500 dark:text-slate-400" role="status">Checking account...</p>
        </Match>
        <Match when={account.user()?.role !== "admin"}>
          <p class="text-sm text-slate-600 dark:text-slate-300">You do not have access to this page.</p>
        </Match>
        <Match when={true}>
          <UsersSection onChanged={() => void account.reload()} />
          <section>
            <h2 class="text-base font-semibold text-slate-900 dark:text-slate-100">Queue</h2>
            <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">Every job the schedule has run, newest first.</p>
            <a
              href="/queue"
              class="mt-2 inline-block text-sm font-medium text-slate-700 underline hover:text-slate-950 dark:text-slate-300 dark:hover:text-white"
            >
              Open the queue
            </a>
          </section>
        </Match>
      </Switch>
    </div>
  );
};
