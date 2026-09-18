import type { JSX } from "@solidjs/web";
import { createSignal, Match, Show, Switch } from "solid-js";

import { logout } from "../api/users";
import { ThemeToggle } from "../components/ThemeToggle";
import { UserAdminDialog } from "../components/UserAdminDialog";
import { AccountProvider, useAccount } from "./account";

type SignOutState = { phase: "ready"; } | { phase: "signing-out"; } | { phase: "error"; message: string; };

const SignOutControl = () => {
  const account = useAccount();
  const [state, setState] = createSignal<SignOutState>({ phase: "ready" });

  const signOut = async (): Promise<void> => {
    setState({ phase: "signing-out" });

    const result = await logout();

    if (!result.ok) {
      setState({ phase: "error", message: result.message });

      return;
    }

    setState({ phase: "ready" });
    await account.reload();
  };

  const failure = (): string | undefined => {
    const current = state();

    return current.phase === "error" ? current.message : undefined;
  };

  return (
    <>
      <button
        type="button"
        disabled={state().phase === "signing-out"}
        onClick={() => void signOut()}
        class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-100 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
      >
        {state().phase === "signing-out" ? "Signing out..." : "Sign out"}
      </button>
      <Show when={failure()}>
        {message => <span class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</span>}
      </Show>
    </>
  );
};

const AccountControls = () => {
  const account = useAccount();

  const loadFailure = (): string | undefined => {
    const current = account.state();

    return current.phase === "error" ? current.message : undefined;
  };

  return (
    <Switch>
      <Match when={account.state().phase === "loading"}>
        <span class="text-sm text-slate-500 dark:text-slate-400" role="status">Checking account...</span>
      </Match>
      <Match when={account.state().phase === "anonymous"}>
        <a href="/auth/github/login" class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
          Sign in
        </a>
      </Match>
      <Match when={loadFailure()}>
        {message => <span class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</span>}
      </Match>
      <Match when={account.user()}>
        {user => (
          <div class="flex items-center gap-3">
            <Show
              when={user().role === "guest"}
              fallback={<span class="max-w-40 truncate text-sm text-slate-600 dark:text-slate-300">{user().display_name}</span>}
            >
              <span class="text-sm text-slate-500 dark:text-slate-400" role="status">Guest access pending</span>
            </Show>
            <Show when={user().role === "admin"}>
              <UserAdminDialog onChanged={() => void account.reload()} />
            </Show>
            <SignOutControl />
          </div>
        )}
      </Match>
    </Switch>
  );
};

const AppShellContent = (properties: { children?: JSX.Element; }) => {
  const account = useAccount();

  return (
    <div class="min-h-screen bg-slate-50 text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <header class="border-b border-slate-200 dark:border-slate-800">
        <div class="mx-auto flex max-w-5xl items-center justify-between gap-4 px-6 py-2">
          <nav class="flex min-w-0 items-center gap-4">
            <a href="/" aria-label="error.menu" class="flex shrink-0 items-center gap-2 text-sm font-semibold tracking-tight">
              <img src="/logo.svg" alt="" class="size-6" />
              error.menu
            </a>
            <a href="/" class="text-sm text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100">
              Projects
            </a>
            <Show when={account.user()}>
              {user => (
                <Show when={user().role === "admin"}>
                  <a href="/queue" class="text-sm text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100">
                    Queue
                  </a>
                </Show>
              )}
            </Show>
          </nav>
          <div class="flex shrink-0 items-center gap-3">
            <AccountControls />
            <ThemeToggle />
          </div>
        </div>
      </header>
      <main class="mx-auto max-w-5xl px-6 py-8">{properties.children}</main>
    </div>
  );
};

export const AppShell = (properties: { children?: JSX.Element; }) => (
  <AccountProvider>
    <AppShellContent>{properties.children}</AppShellContent>
  </AccountProvider>
);
