import { FiArrowLeft, FiFolder, FiImage, FiPlus, FiX } from "solid-icons/fi";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { IconCandidate, TreeEntry } from "../api/projects";
import { listIconCandidates, readTree } from "../api/projects";

export type IconScheme = "light" | "dark";

type BrowseState
  = | { phase: "idle"; }
    | { phase: "loading"; }
    | { phase: "loaded"; path: string; entries: readonly TreeEntry[]; }
    | { phase: "error"; message: string; };

export const blobSource = (projectId: string, path: string): string =>
  `/api/projects/${projectId}/blob?path=${encodeURIComponent(path)}`;

const SLOT = "flex size-20 items-center justify-center rounded-lg border border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-950";
const EMPTY_SLOT = "flex size-20 items-center justify-center rounded-lg border border-dashed border-slate-300 text-slate-400 hover:border-slate-400 hover:text-slate-600 dark:border-slate-600 dark:text-slate-500 dark:hover:border-slate-500 dark:hover:text-slate-300";

const Slot = (properties: {
  label: string;
  projectId: string;
  path: string;
  isActive: boolean;
  onSelect: () => void;
  onClear: () => void;
}) => (
  <div class="space-y-1.5">
    <p class="text-xs font-medium text-slate-600 dark:text-slate-300">{properties.label}</p>
    <div class="relative">
      <Show
        when={properties.path !== ""}
        fallback={(
          <button
            type="button"
            aria-label={`Choose the ${properties.label} icon`}
            onClick={() => properties.onSelect()}
            class={EMPTY_SLOT}
          >
            <FiPlus size={20} />
          </button>
        )}
      >
        <button
          type="button"
          aria-label={`Change the ${properties.label} icon`}
          onClick={() => properties.onSelect()}
          class={SLOT}
        >
          <img src={blobSource(properties.projectId, properties.path)} alt="" class="max-h-14 max-w-14 object-contain" />
        </button>
        <button
          type="button"
          aria-label={`Clear the ${properties.label} icon`}
          onClick={() => properties.onClear()}
          class="absolute -top-1.5 -right-1.5 rounded-full border border-slate-200 bg-white p-0.5 text-slate-500 hover:text-slate-800 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400 dark:hover:text-slate-100"
        >
          <FiX size={12} />
        </button>
      </Show>
      <Show when={properties.isActive}>
        <span class="pointer-events-none absolute inset-0 rounded-lg ring-2 ring-slate-900 dark:ring-slate-100" />
      </Show>
    </div>
    <p class="max-w-20 truncate font-mono text-[10px] text-slate-400 dark:text-slate-500">{properties.path}</p>
  </div>
);

const Thumbnail = (properties: { projectId: string; path: string; label: string; onPick: () => void; }) => (
  <button
    type="button"
    onClick={() => properties.onPick()}
    class="flex w-20 shrink-0 flex-col items-center gap-1 rounded-lg border border-slate-200 p-1.5 hover:border-slate-400 hover:bg-slate-50 dark:border-slate-700 dark:hover:border-slate-500 dark:hover:bg-slate-800"
  >
    <img src={blobSource(properties.projectId, properties.path)} alt="" class="h-10 w-full object-contain" />
    <span class="w-full truncate text-center text-[10px] text-slate-500 dark:text-slate-400">{properties.label}</span>
  </button>
);

export const IconPicker = (properties: {
  projectId: string;
  lightPath: string;
  darkPath: string;
  onChange: (scheme: IconScheme, path: string) => void;
}) => {
  const [active, setActive] = createSignal<IconScheme>("light");
  const [isPicking, setIsPicking] = createSignal(false);
  const [candidates, setCandidates] = createSignal<readonly IconCandidate[]>([]);
  const [browseState, setBrowseState] = createSignal<BrowseState>({ phase: "idle" });

  const entries = (): readonly TreeEntry[] => {
    const current = browseState();

    return current.phase === "loaded" ? current.entries : [];
  };
  const here = (): string => {
    const current = browseState();

    return current.phase === "loaded" ? current.path : "";
  };
  const browseError = (): string | null => {
    const current = browseState();

    return current.phase === "error" ? current.message : null;
  };
  const parent = (): string | undefined => {
    const path = here();

    if (path === "") return undefined;

    return path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  };
  const browse = (path: string): void => {
    setBrowseState({ phase: "loading" });
    void readTree(properties.projectId, path).then((result) => {
      setBrowseState(result.ok
        ? { phase: "loaded", path: result.value.path, entries: result.value.entries }
        : { phase: "error", message: result.message });
    });
  };
  const choose = (path: string): void => {
    properties.onChange(active(), path);
    setIsPicking(false);
  };
  const open = (scheme: IconScheme): void => {
    setActive(scheme);
    setIsPicking(true);
  };

  // Suggestions and the root listing are only worth fetching once the reader asks to pick.
  createEffect(
    () => isPicking(),
    (picking) => {
      if (!picking || candidates().length > 0) return;

      void listIconCandidates(properties.projectId).then((result) => {
        if (result.ok) setCandidates(result.value);
      });
      browse("");
    },
  );

  return (
    <div class="space-y-3">
      <div class="flex gap-4">
        <Slot
          label="Light"
          projectId={properties.projectId}
          path={properties.lightPath}
          isActive={isPicking() && active() === "light"}
          onSelect={() => open("light")}
          onClear={() => properties.onChange("light", "")}
        />
        <Slot
          label="Dark"
          projectId={properties.projectId}
          path={properties.darkPath}
          isActive={isPicking() && active() === "dark"}
          onSelect={() => open("dark")}
          onClear={() => properties.onChange("dark", "")}
        />
      </div>

      <Show when={isPicking()}>
        <div class="space-y-3 rounded-lg border border-slate-200 p-3 dark:border-slate-800">
          <p class="text-xs text-slate-500 dark:text-slate-400">
            {`Choosing the ${active()} icon.`}
          </p>

          <Show when={candidates().length > 0}>
            <div class="space-y-1.5">
              <p class="text-xs font-medium text-slate-600 dark:text-slate-300">Suggested</p>
              <div class="flex gap-2 overflow-x-auto pb-1">
                <For each={candidates().slice(0, 12)}>
                  {candidate => (
                    <Thumbnail
                      projectId={properties.projectId}
                      path={candidate.path}
                      label={candidate.path.split("/").pop() ?? candidate.path}
                      onPick={() => choose(candidate.path)}
                    />
                  )}
                </For>
              </div>
            </div>
          </Show>

          <div class="space-y-1.5">
            <div class="flex items-center gap-2">
              <button
                type="button"
                disabled={parent() === undefined}
                onClick={() => browse(parent() ?? "")}
                aria-label="Up one directory"
                class="rounded border border-slate-200 p-1 text-slate-500 hover:bg-slate-100 disabled:opacity-40 dark:border-slate-700 dark:text-slate-400 dark:hover:bg-slate-800"
              >
                <FiArrowLeft size={12} />
              </button>
              <p class="truncate font-mono text-xs text-slate-500 dark:text-slate-400">{here() === "" ? "/" : here()}</p>
            </div>
            <Show when={browseState().phase === "loading"}>
              <p class="text-xs text-slate-500 dark:text-slate-400">Reading...</p>
            </Show>
            <Show when={browseError()}>
              {message => <p class="text-xs text-red-600 dark:text-red-400">{message()}</p>}
            </Show>
            <ul class="max-h-56 divide-y divide-slate-100 overflow-y-auto rounded-md border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
              <For each={entries()} fallback={<li class="px-2 py-3 text-center text-xs text-slate-400 dark:text-slate-500">Nothing here.</li>}>
                {entry => (
                  <li>
                    <button
                      type="button"
                      disabled={!entry.is_directory && !entry.is_image}
                      onClick={() => (entry.is_directory ? browse(entry.path) : choose(entry.path))}
                      class="flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs text-slate-700 hover:bg-slate-100 disabled:text-slate-300 disabled:hover:bg-transparent dark:text-slate-200 dark:hover:bg-slate-800 dark:disabled:text-slate-600"
                    >
                      <Show when={entry.is_directory} fallback={<FiImage size={12} />}>
                        <FiFolder size={12} />
                      </Show>
                      <span class="truncate">{entry.name}</span>
                      <Show when={entry.is_image}>
                        <img
                          src={blobSource(properties.projectId, entry.path)}
                          alt=""
                          class="ml-auto h-5 w-8 shrink-0 object-contain"
                        />
                      </Show>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </div>

          <div class="flex justify-end">
            <button
              type="button"
              onClick={() => setIsPicking(false)}
              class="text-xs text-slate-500 underline hover:text-slate-800 dark:text-slate-400 dark:hover:text-slate-200"
            >
              Done
            </button>
          </div>
        </div>
      </Show>
    </div>
  );
};
