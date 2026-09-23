import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Tooltip } from "@kobalte/core/tooltip";
import type { JSX } from "@solidjs/web";
import { FiActivity, FiBookOpen, FiCheck, FiChevronDown, FiKey, FiLayers, FiLogOut, FiShield, FiUsers } from "solid-icons/fi";
import { createEffect, createSignal, For, Match, Show, Switch, untrack } from "solid-js";

import type { HealthState } from "../api/health";
import { fetchHealth } from "../api/health";
import type { Organization } from "../api/organizations";
import { listOrganizations } from "../api/organizations";
import type { User } from "../api/users";
import { logout } from "../api/users";
import { ApiTokensDialog } from "../components/ApiTokensDialog";
import { ThemeToggle } from "../components/ThemeToggle";
import { AccountProvider, useAccount } from "./account";
import { ScopeProvider, useScope } from "./scope";

type SignOutState = { phase: "ready"; } | { phase: "signing-out"; } | { phase: "error"; message: string; };

const MENU_ITEM_BASE = "flex items-center gap-2 rounded-control px-2 py-1.5 text-sm no-underline outline-none select-none data-[highlighted]:bg-raised data-[disabled]:cursor-not-allowed data-[disabled]:opacity-60";
const MENU_ITEM = `${MENU_ITEM_BASE} text-slate-700 dark:text-slate-300`;
const MENU_ITEM_DANGER = `${MENU_ITEM_BASE} text-red-600 dark:text-red-400`;
const MENU_TRIGGER = "flex max-w-48 items-center gap-1.5 rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300";
const GLYPH = "flex size-8 items-center justify-center rounded-control text-slate-500 hover:bg-raised hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100";
const TOOLTIP = "z-50 max-w-xs rounded-control bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900";
const ALL_ORGANIZATIONS = "all";

const GlyphLink = (properties: { href: string; label: string; children: JSX.Element; }) => (
  <Tooltip>
    <Tooltip.Trigger
      as="a"
      href={properties.href}
      aria-label={properties.label}
      class={GLYPH}
    >
      {properties.children}
    </Tooltip.Trigger>
    <Tooltip.Portal>
      <Tooltip.Content class={TOOLTIP}>
        <Tooltip.Arrow />
        {properties.label}
      </Tooltip.Content>
    </Tooltip.Portal>
  </Tooltip>
);

const HealthGlyph = () => {
  const [health, setHealth] = createSignal<HealthState>({ phase: "loading" });

  createEffect(
    () => undefined,
    () => {
      void (async () => {
        setHealth(await fetchHealth());
      })();
    },
  );

  const label = (): string => {
    const current = health();

    if (current.phase === "loading") return "Checking health";

    if (current.phase === "error") return `Health check failed: ${current.message}`;

    return `Health: ${current.health.status}, version ${current.health.version}`;
  };

  const tone = (): string => {
    const current = health();

    if (current.phase === "error") return "text-red-600 dark:text-red-400";

    if (current.phase === "loaded" && current.health.status === "ok") return "text-emerald-600 dark:text-emerald-400";

    return "";
  };

  return (
    <GlyphLink href="/health" label={label()}>
      <span class={tone()}>
        <FiActivity size={16} />
      </span>
    </GlyphLink>
  );
};

const ScopeSwitcher = () => {
  const scope = useScope();
  const [organizations, setOrganizations] = createSignal<readonly Organization[]>([]);

  createEffect(
    () => undefined,
    () => {
      void (async () => {
        const result = await listOrganizations();

        if (!result.ok) return;

        setOrganizations(result.value);

        const chosen = untrack(() => scope.organizationId());

        if (chosen !== null && result.value.every(organization => organization.organization_id !== chosen)) scope.choose(null);
      })();
    },
  );

  const label = (): string =>
    organizations().find(organization => organization.organization_id === scope.organizationId())?.name ?? "All organizations";

  return (
    <DropdownMenu placement="bottom-start" gutter={6}>
      <DropdownMenu.Trigger class={MENU_TRIGGER} aria-label={`Organization filter: ${label()}`}>
        <span class="truncate">{label()}</span>
        <DropdownMenu.Icon class="shrink-0 text-slate-400 dark:text-slate-500">
          <FiChevronDown size={14} />
        </DropdownMenu.Icon>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="z-50 w-56 rounded-panel bg-surface p-1 shadow-xl">
          <DropdownMenu.RadioGroup
            value={scope.organizationId() ?? ALL_ORGANIZATIONS}
            onChange={value => scope.choose(value === ALL_ORGANIZATIONS ? null : value)}
          >
            <DropdownMenu.RadioItem value={ALL_ORGANIZATIONS} closeOnSelect class={MENU_ITEM}>
              <span class="flex-1 truncate">All organizations</span>
              <DropdownMenu.ItemIndicator>
                <FiCheck size={14} />
              </DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
            <For each={organizations()}>
              {organization => (
                <DropdownMenu.RadioItem value={organization.organization_id} closeOnSelect class={MENU_ITEM}>
                  <span class="flex-1 truncate">{organization.name}</span>
                  <DropdownMenu.ItemIndicator>
                    <FiCheck size={14} />
                  </DropdownMenu.ItemIndicator>
                </DropdownMenu.RadioItem>
              )}
            </For>
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
};

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
        <DropdownMenu.Trigger class={MENU_TRIGGER}>
          <span class="truncate">{properties.user.display_name}</span>
          <DropdownMenu.Icon class="shrink-0 text-slate-400 dark:text-slate-500">
            <FiChevronDown size={14} />
          </DropdownMenu.Icon>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class="z-50 w-56 rounded-panel bg-surface p-1 shadow-xl">
            <DropdownMenu.Group>
              <DropdownMenu.GroupLabel as="div" class="px-2 py-1.5">
                <p class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{properties.user.display_name}</p>
                <p class="text-xs text-slate-500 dark:text-slate-400">{properties.user.role}</p>
              </DropdownMenu.GroupLabel>
              <DropdownMenu.Item as="a" href="/orgs" class={MENU_ITEM}>
                <FiUsers size={14} />
                Organizations
              </DropdownMenu.Item>
              <DropdownMenu.Item class={MENU_ITEM} onSelect={() => setAreTokensOpen(true)}>
                <FiKey size={14} />
                API tokens
              </DropdownMenu.Item>
            </DropdownMenu.Group>
            <Show when={properties.user.role === "admin"}>
              <DropdownMenu.Separator class="my-1 border-hairline" />
              <DropdownMenu.Group>
                <DropdownMenu.Item as="a" href="/admin" class={MENU_ITEM}>
                  <FiShield size={14} />
                  Admin
                </DropdownMenu.Item>
                <DropdownMenu.Item as="a" href="/queue" class={MENU_ITEM}>
                  <FiLayers size={14} />
                  Queue
                </DropdownMenu.Item>
              </DropdownMenu.Group>
            </Show>
            <DropdownMenu.Separator class="my-1 border-hairline" />
            <DropdownMenu.Item
              class={MENU_ITEM_DANGER}
              disabled={signOutState().phase === "signing-out"}
              onSelect={() => void signOut()}
            >
              <FiLogOut size={14} />
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
        <a href="/auth/github/login" rel="external" class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
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

const SignedInNavigation = () => {
  const account = useAccount();

  return (
    <Show when={account.user()}>
      <span class="text-slate-300 dark:text-slate-600" aria-hidden="true">/</span>
      <ScopeSwitcher />
    </Show>
  );
};

const AdminGlyphs = () => {
  const account = useAccount();

  return (
    <Show when={account.user()?.role === "admin"}>
      <HealthGlyph />
    </Show>
  );
};

const AppShellContent = (properties: { children?: JSX.Element; }) => (
  <div class="min-h-screen text-slate-900 dark:text-slate-100">
    <header>
      <div class="mx-auto flex max-w-5xl items-center justify-between gap-4 px-6 py-2">
        <nav class="flex min-w-0 items-center gap-3" aria-label="Main">
          <a href="/" aria-label="error.menu" class="flex shrink-0 items-center gap-2 text-sm font-semibold tracking-tight">
            <img src="/logo.svg" alt="" class="size-6" />
            error.menu
          </a>
          <SignedInNavigation />
        </nav>
        <div class="flex shrink-0 items-center gap-1">
          <AdminGlyphs />
          <GlyphLink href="/docs" label="API reference">
            <FiBookOpen size={16} />
          </GlyphLink>
          <div class="ml-2 flex items-center gap-3">
            <AccountControls />
            <ThemeToggle />
          </div>
        </div>
      </div>
    </header>
    <main class="mx-auto max-w-5xl px-6 py-8">{properties.children}</main>
  </div>
);

export const AppShell = (properties: { children?: JSX.Element; }) => (
  <AccountProvider>
    <ScopeProvider>
      <AppShellContent>{properties.children}</AppShellContent>
    </ScopeProvider>
  </AccountProvider>
);
