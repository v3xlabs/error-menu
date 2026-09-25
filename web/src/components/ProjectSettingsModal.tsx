import { Dialog } from "@kobalte/core/dialog";
import { Tabs } from "@kobalte/core/tabs";
import { FiSettings } from "solid-icons/fi";
import { createEffect, createSignal, Show } from "solid-js";

import type { Project } from "../api/projects";
import { describeProject, renameProject, setProjectAnalyzers, setProjectIcon } from "../api/projects";
import type { AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { AnalyzerSelect } from "./AnalyzerSelect";
import { IconPicker } from "./IconPicker";
import { InfoTip } from "./InfoTip";
import { ProjectCustody } from "./ProjectCustody";
import { ProjectMembers } from "./ProjectMembers";
import { ProjectReporting } from "./ProjectReporting";
import { TAB_LIST, TAB_TRIGGER } from "./Tabs";

type SaveState = { phase: "ready"; } | { phase: "saving"; } | { phase: "error"; message: string; };

const DEFAULT_ANALYZERS: readonly AnalyzerId[] = ANALYZERS.map(analyzer => analyzer.analyzerId);

const FIELD = "w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 dark:text-slate-100";

export const ProjectSettingsModal = (properties: { project: Project; onSaved: () => void; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [name, setName] = createSignal("");
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
        name: properties.project.name,
        description: properties.project.description ?? "",
        lightPath: properties.project.icon_light_source ?? "",
        darkPath: properties.project.icon_dark_source ?? "",
        usesDefaultAnalyzers: properties.project.uses_default_analyzers,
        enabledAnalyzers: properties.project.analyzers,
      };
    },
    (project) => {
      if (project === null) return;

      setName(project.name);
      setDescription(project.description);
      setLightPath(project.lightPath);
      setDarkPath(project.darkPath);
      setUsesDefaultAnalyzers(project.usesDefaultAnalyzers);
      setEnabledAnalyzers(project.enabledAnalyzers);
    },
  );

  const save = async (): Promise<void> => {
    const nextName = name().trim();

    if (nextName.length === 0) {
      setSaveState({ phase: "error", message: "Enter a project name." });

      return;
    }

    setSaveState({ phase: "saving" });

    if (nextName !== properties.project.name) {
      const renamed = await renameProject(properties.project.project_id, nextName);

      if (!renamed.ok) {
        setSaveState({ phase: "error", message: renamed.message });

        return;
      }
    }

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
                <Tabs.Trigger value="github" class={TAB_TRIGGER}>GitHub</Tabs.Trigger>
                <Tabs.Trigger value="danger" class={TAB_TRIGGER}>Danger</Tabs.Trigger>
              </Tabs.List>

              <Tabs.Content value="info" class="min-h-0 flex-1 space-y-4 overflow-y-auto pt-4">
                <div class="space-y-1.5">
                  <label for="project-name" class="block text-sm font-medium text-slate-700 dark:text-slate-300">
                    Name
                  </label>
                  <input
                    id="project-name"
                    type="text"
                    value={name()}
                    onInput={event => setName(event.currentTarget.value)}
                    class={FIELD}
                  />
                </div>
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

              <Tabs.Content value="github" class="min-h-0 flex-1 overflow-y-auto pt-4">
                <ProjectReporting projectId={properties.project.project_id} />
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
