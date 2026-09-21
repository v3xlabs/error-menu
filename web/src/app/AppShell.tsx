import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import type { JSX } from "@solidjs/web";
import { FiChevronDown } from "solid-icons/fi";
import { createSignal, Match, Show, Switch } from "solid-js";

import type { User } from "../api/users";
import { logout } from "../api/users";
import { ApiTokensDialog } from "../components/ApiTokensDialog";
import { ThemeToggle } from "../components/ThemeToggle";
import { AccountProvider, useAccount } from "./account";

type SignOutState = { phase: "ready"; } | { phase: "signing-out"; } | { phase: "error"; message: string; };

const MENU_ITEM = "flex cursor-default items-center rounded-md px-2 py-1.5 text-sm text-slate-700 no-underline outline-none select-none data-[highlighted]:bg-slate-100 data-[disabled]:cursor-not-allowed data-[disabled]:opacity-60 dark:text-slate-300 dark:data-[highlighted]:bg-slate-800";

const AccountMenu = (properties: { user: User; }) => {
  const account = useAccount();
  const [areTokensOpen, setAreTokensOpen] = createSignal(false);
  const [signOutState, setSignOutState] = createSignal<SignOutState>({ phase: "ready" });

  const signOut = async (): Promise<void> => {
    setSignOutState({ phase: "signing-out" });

    const result = await logout();

    if (!result.ok) {
      setSignOutState({ phase: "error", message: result.message });

      return;
    }

    setSignOutState({ phase: "ready" });
    await account.reload();
  };

  const signOutFailure = (): string | undefined => {
    const current = signOutState();

    return current.phase === "error" ? current.message : undefined;
  };

  return (
    <div class="flex items-center gap-3">
      <Show when={properties.user.role === "guest"}>
        <span class="text-sm text-slate-500 dark:text-slate-400" role="status">Guest access pending</span>
      </Show>
      <Show when={signOutFailure()}>
        {message => <span class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</span>}
      </Show>
      <DropdownMenu placement="bottom-end" gutter={6}>
        <DropdownMenu.Trigger class="flex max-w-48 items-center gap-1.5 rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-100 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800">
          <span class="truncate">{properties.user.display_name}</span>
          <DropdownMenu.Icon class="shrink-0 text-slate-400 dark:text-slate-500">
            <FiChevronDown size={14} />
          </DropdownMenu.Icon>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class="z-50 w-56 rounded-lg border border-slate-200 bg-white p-1 shadow-xl dark:border-slate-800 dark:bg-slate-900">
            <DropdownMenu.Group>
              <DropdownMenu.GroupLabel as="div" class="px-2 py-1.5">
                <p class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{properties.user.display_name}</p>
                <p class="text-xs text-slate-500 dark:text-slate-400">{properties.user.role}</p>
              </DropdownMenu.GroupLabel>
              <DropdownMenu.Item class={MENU_ITEM} onSelect={() => setAreTokensOpen(true)}>
                API tokens
              </DropdownMenu.Item>
              <Show when={properties.user.role === "admin"}>
                <DropdownMenu.Item as="a" href="/admin" class={MENU_ITEM}>
                  Admin
                </DropdownMenu.Item>
              </Show>
            </DropdownMenu.Group>
            <DropdownMenu.Separator class="my-1 border-slate-200 dark:border-slate-800" />
            <DropdownMenu.Item
              class={MENU_ITEM}
              disabled={signOutState().phase === "signing-out"}
              onSelect={() => void signOut()}
            >
              {signOutState().phase === "signing-out" ? "Signing out..." : "Sign out"}
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu>
      <ApiTokensDialog open={areTokensOpen()} onOpenChange={setAreTokensOpen} />
    </div>
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
        {user => <AccountMenu user={user()} />}
      </Match>
    </Switch>
  );
};

const AppShellContent = (properties: { children?: JSX.Element; }) => (
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

export const AppShell = (properties: { children?: JSX.Element; }) => (
  <AccountProvider>
    <AppShellContent>{properties.children}</AppShellContent>
  </AccountProvider>
);
