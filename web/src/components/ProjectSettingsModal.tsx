import { Dialog } from "@kobalte/core/dialog";
import { FiSettings } from "solid-icons/fi";
import { createEffect, createSignal, Show } from "solid-js";

import type { Project } from "../api/projects";
import { describeProject, setProjectAnalyzers, setProjectIcon } from "../api/projects";
import type { AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { AnalyzerSelect } from "./AnalyzerSelect";
import { IconPicker } from "./IconPicker";
import { InfoTip } from "./InfoTip";

type SaveState = { phase: "ready"; } | { phase: "saving"; } | { phase: "error"; message: string; };

const DEFAULT_ANALYZERS: readonly AnalyzerId[] = ANALYZERS.map(analyzer => analyzer.analyzerId);

const FIELD = "w-full rounded-md border border-slate-300 px-3 py-1.5 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";

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

  const save = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();
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
        class="rounded-md border border-slate-300 p-2 text-slate-600 hover:bg-slate-100 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800"
      >
        <FiSettings size={16} />
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-lg border border-slate-200 bg-white p-5 shadow-xl dark:border-slate-800 dark:bg-slate-900">
            <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Project settings</Dialog.Title>
            <form class="mt-4 space-y-4" onSubmit={event => void save(event)}>
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

              <fieldset class="space-y-3">
                <legend class="text-sm font-medium text-slate-700 dark:text-slate-300">Analyzer configuration</legend>
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
                <div class="space-y-1.5">
                  <p class="text-sm font-medium text-slate-700 dark:text-slate-300">Enabled analyzers</p>
                  <AnalyzerSelect selected={selectedAnalyzers()} onToggle={toggleSelectedAnalyzer} />
                </div>
              </fieldset>

              <Show when={saveError()}>
                {message => <p class="text-sm text-red-600 dark:text-red-400">{message()}</p>}
              </Show>
              <div class="flex justify-end gap-2">
                <Dialog.CloseButton class="rounded-md border border-slate-300 px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-300 dark:hover:bg-slate-800">
                  Cancel
                </Dialog.CloseButton>
                <button
                  type="submit"
                  disabled={saveState().phase === "saving"}
                  class="rounded-md bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
                >
                  {saveState().phase === "saving" ? "Saving..." : "Save"}
                </button>
              </div>
            </form>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
