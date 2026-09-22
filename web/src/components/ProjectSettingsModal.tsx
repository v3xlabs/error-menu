import { Dialog } from "@kobalte/core/dialog";
import { Popover } from "@kobalte/core/popover";
import { Tabs } from "@kobalte/core/tabs";
import { FiSettings } from "solid-icons/fi";
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";

import type { Project, ProjectMember, ProjectRole } from "../api/projects";
import {
  describeProject,
  listProjectMembers,
  removeProjectMember,
  setProjectAnalyzers,
  setProjectIcon,
  setProjectMember,
} from "../api/projects";
import type { User } from "../api/users";
import { listUsers } from "../api/users";
import type { AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { AnalyzerSelect } from "./AnalyzerSelect";
import { IconPicker } from "./IconPicker";
import { InfoTip } from "./InfoTip";
import { ProjectCustody } from "./ProjectCustody";
import { TAB_LIST, TAB_TRIGGER } from "./Tabs";

type SaveState = { phase: "ready"; } | { phase: "saving"; } | { phase: "error"; message: string; };
type MembersState
  = | { phase: "loading"; }
    | { phase: "loaded"; members: readonly ProjectMember[]; }
    | { phase: "error"; message: string; };
type UsersState
  = | { phase: "loading"; }
    | { phase: "loaded"; users: readonly User[]; }
    | { phase: "error"; message: string; };
type MemberWriteState = { phase: "ready"; } | { phase: "busy"; userId: string; } | { phase: "error"; message: string; };

const DEFAULT_ANALYZERS: readonly AnalyzerId[] = ANALYZERS.map(analyzer => analyzer.analyzerId);

const FIELD = "w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100";

const PROJECT_ROLES: readonly ProjectRole[] = ["viewer", "operator", "owner"];

const ProjectMembers = (properties: { projectId: string; }) => {
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

export const ProjectSettingsModal = (properties: { project: Project; onSaved: () => void; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [description, setDescription] = createSignal("");
  const [lightPath, setLightPath] = createSignal("");
  const [darkPath, setDarkPath] = createSignal("");
  const [usesDefaultAnalyzers, setUsesDefaultAnalyzers] = createSignal(true);
  const [enabledAnalyzers, setEnabledAnalyzers] = createSignal<readonly AnalyzerId[]>([]);
  const [saveState, setSaveState] = createSignal<SaveState>({ phase: "ready" });

  const selectedAnalyzers = (): readonly AnalyzerId[] =>
    (usesDefaultAnalyzers() ? DEFAULT_ANALYZERS : enabledAnalyzers());

  const toggleSelectedAnalyzer = (analyzerId: AnalyzerId): void => {
    const current = selectedAnalyzers();

    setUsesDefaultAnalyzers(false);
    setEnabledAnalyzers(
      current.includes(analyzerId) ? current.filter(analyzer => analyzer !== analyzerId) : [...current, analyzerId],
    );
  };

  const saveError = (): string | null => {
    const current = saveState();

    return current.phase === "error" ? current.message : null;
  };

  createEffect(
    () => {
      if (!isOpen()) return null;

      return {
        description: properties.project.description ?? "",
        lightPath: properties.project.icon_light_source ?? "",
        darkPath: properties.project.icon_dark_source ?? "",
        usesDefaultAnalyzers: properties.project.uses_default_analyzers,
        enabledAnalyzers: properties.project.analyzers,
      };
    },
    (project) => {
      if (project === null) return;

      setDescription(project.description);
      setLightPath(project.lightPath);
      setDarkPath(project.darkPath);
      setUsesDefaultAnalyzers(project.usesDefaultAnalyzers);
      setEnabledAnalyzers(project.enabledAnalyzers);
    },
  );

  const save = async (): Promise<void> => {
    setSaveState({ phase: "saving" });

    const described = await describeProject(
      properties.project.project_id,
      description().trim() === "" ? null : description().trim(),
    );

    if (!described.ok) {
      setSaveState({ phase: "error", message: described.message });

      return;
    }

    const iconed = await setProjectIcon(properties.project.project_id, {
      light_path: lightPath().trim(),
      dark_path: darkPath().trim(),
    });

    if (!iconed.ok) {
      setSaveState({ phase: "error", message: iconed.message });

      return;
    }

    const analyzers = await setProjectAnalyzers(properties.project.project_id, {
      uses_default_analyzers: usesDefaultAnalyzers(),
      analyzers: [...selectedAnalyzers()],
    });

    if (!analyzers.ok) {
      setSaveState({ phase: "error", message: analyzers.message });

      return;
    }

    setSaveState({ phase: "ready" });
    setIsOpen(false);
    properties.onSaved();
  };

  return (
    <Dialog open={isOpen()} onOpenChange={setIsOpen}>
      <Dialog.Trigger
        aria-label="Project settings"
        class="rounded-control bg-raised p-2 text-slate-600 hover:bg-raised-hover dark:text-slate-300"
      >
        <FiSettings size={16} />
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="flex h-128 max-h-[85vh] w-full max-w-lg flex-col rounded-panel bg-surface p-5 shadow-xl">
            <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Project settings</Dialog.Title>
            <Tabs class="mt-4 flex min-h-0 flex-1 flex-col">
              <Tabs.List class={TAB_LIST}>
                <Tabs.Trigger value="info" class={TAB_TRIGGER}>Info</Tabs.Trigger>
                <Tabs.Trigger value="analyzers" class={TAB_TRIGGER}>Analyzers</Tabs.Trigger>
                <Tabs.Trigger value="access" class={TAB_TRIGGER}>Access</Tabs.Trigger>
                <Tabs.Trigger value="danger" class={TAB_TRIGGER}>Danger</Tabs.Trigger>
              </Tabs.List>

              <Tabs.Content value="info" class="min-h-0 flex-1 space-y-4 overflow-y-auto pt-4">
                <div class="space-y-1.5">
                  <label for="project-description" class="flex items-center gap-1.5 text-sm font-medium text-slate-700 dark:text-slate-300">
                    Description
                    <InfoTip
                      label="Where the description comes from"
                      text="Written by hand today. A reviewing model will keep it current later."
                    />
                  </label>
                  <textarea
                    id="project-description"
                    rows="4"
                    value={description()}
                    onInput={event => setDescription(event.currentTarget.value)}
                    placeholder="What this project is for, in a paragraph."
                    class={FIELD}
                  />
                </div>

                <div class="space-y-2">
                  <p class="text-sm font-medium text-slate-700 dark:text-slate-300">Icon</p>
                  <IconPicker
                    projectId={properties.project.project_id}
                    lightPath={lightPath()}
                    darkPath={darkPath()}
                    onChange={(scheme, path) => (scheme === "light" ? setLightPath(path) : setDarkPath(path))}
                  />
                </div>
              </Tabs.Content>

              <Tabs.Content value="analyzers" class="min-h-0 flex-1 space-y-3 overflow-y-auto pt-4">
                <div class="space-y-1.5">
                  <label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
                    <input
                      type="radio"
                      name="project-analyzer-mode"
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
                      name="project-analyzer-mode"
                      checked={!usesDefaultAnalyzers()}
                      onInput={() => setUsesDefaultAnalyzers(false)}
                    />
                    <span class="font-medium">Choose analyzers</span>
                  </label>
                </div>
                <AnalyzerSelect selected={selectedAnalyzers()} onToggle={toggleSelectedAnalyzer} />
              </Tabs.Content>

              <Tabs.Content value="access" class="min-h-0 flex-1 overflow-y-auto pt-4">
                <ProjectMembers projectId={properties.project.project_id} />
              </Tabs.Content>

              <Tabs.Content value="danger" class="min-h-0 flex-1 overflow-y-auto pt-4">
                <ProjectCustody project={properties.project} onMoved={() => properties.onSaved()} />
              </Tabs.Content>
            </Tabs>
            <Show when={saveError()}>
              {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400">{message()}</p>}
            </Show>
            <div class="mt-5 flex justify-end gap-2 border-t border-hairline pt-4">
              <Dialog.CloseButton class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300">
                Cancel
              </Dialog.CloseButton>
              <button
                type="button"
                disabled={saveState().phase === "saving"}
                onClick={() => void save()}
                class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
              >
                {saveState().phase === "saving" ? "Saving..." : "Save"}
              </button>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
