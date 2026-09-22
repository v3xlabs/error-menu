import { Dialog } from "@kobalte/core/dialog";
import { createSignal, Show } from "solid-js";

import { runAnalysis, scanChange } from "../api/projects";
import type { InspectTarget } from "../domain/inspect";
import { parseInspectInput } from "../domain/inspect";
import { InfoTip } from "./InfoTip";

type Mode = "link" | "range";
type SubmitState = { phase: "ready"; } | { phase: "running"; } | { phase: "error"; message: string; };

const FIELD = "w-full rounded-control bg-raised px-3 py-1.5 font-mono text-sm text-slate-900 dark:text-slate-100";
const MODE = "px-2.5 py-1 text-xs font-medium text-slate-600 hover:bg-raised-hover dark:text-slate-300";
const MODE_SELECTED = "bg-slate-900 px-2.5 py-1 text-xs font-medium text-white dark:bg-slate-100 dark:text-slate-900";
const UNREADABLE = "Paste a link to a commit, a pull request, or a comparison, or a full 40 character commit sha.";

export const InspectModal = (properties: { projectId: string; onAnalysed: () => void; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const [mode, setMode] = createSignal<Mode>("link");
  const [link, setLink] = createSignal("");
  const [baseSha, setBaseSha] = createSignal("");
  const [headSha, setHeadSha] = createSignal("");
  const [submitState, setSubmitState] = createSignal<SubmitState>({ phase: "ready" });

  const submitError = (): string | null => {
    const current = submitState();

    return current.phase === "error" ? current.message : null;
  };

  const failureOf = async (target: InspectTarget): Promise<string | null> => {
    if (target.kind === "change") {
      const scan = await scanChange(properties.projectId, target.number);

      return scan.ok ? null : scan.message;
    }

    const analysis = target.kind === "range"
      ? await runAnalysis(properties.projectId, { base_sha: target.base, head_sha: target.head })
      : await runAnalysis(properties.projectId, { head_sha: target.head });

    return analysis.ok ? null : analysis.message;
  };

  const handleSubmit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const target = parseInspectInput(
      mode() === "link" ? link() : `${baseSha().trim()}...${headSha().trim()}`,
    );

    if (target === null) {
      setSubmitState({ phase: "error", message: UNREADABLE });

      return;
    }

    setSubmitState({ phase: "running" });

    const failure = await failureOf(target);

    if (failure !== null) {
      setSubmitState({ phase: "error", message: failure });

      return;
    }

    setSubmitState({ phase: "ready" });
    setIsOpen(false);
    properties.onAnalysed();
  };

  return (
    <Dialog open={isOpen()} onOpenChange={setIsOpen}>
      <Dialog.Trigger class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300">
        Inspect
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="w-full max-w-lg rounded-panel bg-surface p-5 shadow-xl">
            <div class="flex items-center justify-between gap-4">
              <div class="flex items-center gap-1.5">
                <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">Inspect</Dialog.Title>
                <InfoTip
                  label="What inspect reads"
                  text="A commit, pull request or comparison link is read for the subject it names. A commit without a base is compared against its own first parent. Results stay local to error.menu."
                />
              </div>
              <div class="flex overflow-hidden rounded-control bg-raised">
                <button type="button" onClick={() => setMode("link")} class={mode() === "link" ? MODE_SELECTED : MODE}>
                  Link
                </button>
                <button type="button" onClick={() => setMode("range")} class={mode() === "range" ? MODE_SELECTED : MODE}>
                  Commit range
                </button>
              </div>
            </div>
            <form class="mt-4 grid gap-3" onSubmit={event => void handleSubmit(event)}>
              <Show
                when={mode() === "link"}
                fallback={(
                  <>
                    <div class="space-y-1.5">
                      <label for="base-sha" class="block text-sm font-medium text-slate-700 dark:text-slate-300">Base commit sha</label>
                      <input
                        id="base-sha"
                        required
                        type="text"
                        value={baseSha()}
                        onInput={event => setBaseSha(event.currentTarget.value)}
                        placeholder="The commit before this change"
                        class={FIELD}
                      />
                    </div>
                    <div class="space-y-1.5">
                      <label for="head-sha" class="block text-sm font-medium text-slate-700 dark:text-slate-300">Head commit sha</label>
                      <input
                        id="head-sha"
                        required
                        type="text"
                        value={headSha()}
                        onInput={event => setHeadSha(event.currentTarget.value)}
                        placeholder="The commit to inspect"
                        class={FIELD}
                      />
                    </div>
                  </>
                )}
              >
                <div class="space-y-1.5">
                  <label for="inspect-link" class="block text-sm font-medium text-slate-700 dark:text-slate-300">Link or commit sha</label>
                  <input
                    id="inspect-link"
                    required
                    type="text"
                    value={link()}
                    onInput={event => setLink(event.currentTarget.value)}
                    placeholder="https://github.com/acme/example/pull/42"
                    class={FIELD}
                  />
                </div>
              </Show>
              <Show when={submitError()}>{message => <p class="text-sm text-red-600 dark:text-red-400">{message()}</p>}</Show>
              <div class="flex justify-end gap-2">
                <Dialog.CloseButton class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300">
                  Cancel
                </Dialog.CloseButton>
                <button type="submit" disabled={submitState().phase === "running"} class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white">
                  {submitState().phase === "running" ? "Running..." : "Run analysis"}
                </button>
              </div>
            </form>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
