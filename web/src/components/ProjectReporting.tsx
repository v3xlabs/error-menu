import { createEffect, createSignal, Match, Show, Switch } from "solid-js";

import type { ForgeReporting } from "../api/projects";
import { readForgeReporting, setForgeReporting } from "../api/projects";

type ReportingState
  = | { phase: "loading"; }
    | { phase: "loaded"; reporting: ForgeReporting; }
    | { phase: "error"; message: string; };
type WriteState = { phase: "ready"; } | { phase: "busy"; } | { phase: "error"; message: string; };

const NOTE = "text-sm text-slate-600 dark:text-slate-400";

export const ProjectReporting = (properties: { projectId: string; }) => {
  const [reportingState, setReportingState] = createSignal<ReportingState>({ phase: "loading" });
  const [writeState, setWriteState] = createSignal<WriteState>({ phase: "ready" });

  createEffect(
    () => properties.projectId,
    (projectId) => {
      setReportingState({ phase: "loading" });
      setWriteState({ phase: "ready" });
      void readForgeReporting(projectId).then(result => setReportingState(result.ok
        ? { phase: "loaded", reporting: result.value }
        : { phase: "error", message: result.message }));
    },
  );

  const toggle = async (isEnabled: boolean): Promise<void> => {
    setWriteState({ phase: "busy" });

    const result = await setForgeReporting(properties.projectId, isEnabled);

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return;
    }

    setReportingState({ phase: "loaded", reporting: result.value });
    setWriteState({ phase: "ready" });
  };

  const reporting = (): ForgeReporting | undefined => {
    const current = reportingState();

    return current.phase === "loaded" ? current.reporting : undefined;
  };

  const errorMessage = (): string | undefined => {
    const read = reportingState();

    if (read.phase === "error") return read.message;

    const write = writeState();

    return write.phase === "error" ? write.message : undefined;
  };

  return (
    <section class="space-y-3">
      <Show when={reportingState().phase === "loading"}>
        <p class="text-sm text-slate-500 dark:text-slate-400" role="status">Loading...</p>
      </Show>
      <Show when={errorMessage()}>
        {message => <p class="text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
      </Show>
      <Show when={reporting()}>
        {current => (
          <>
            <label class="flex items-center gap-2 text-sm font-medium text-slate-700 dark:text-slate-300">
              <input
                type="checkbox"
                checked={current().enabled}
                disabled={current().state === "unavailable" || writeState().phase === "busy"}
                onInput={event => void toggle(event.currentTarget.checked)}
              />
              Report to GitHub
            </label>
            <p class={NOTE}>
              Pull requests and pushes to the default branch get an error.menu check. It shows finding counts and a link
              here, and it turns red only for a new High or Critical finding nobody has triaged.
            </p>
            <Switch>
              <Match when={current().state === "unavailable"}>
                <p class={NOTE}>This server has no GitHub App, or the repository is not on github.com.</p>
              </Match>
              <Match when={current().state === "not_installed"}>
                <p class={NOTE}>
                  The error.menu GitHub App is not installed on this repository yet. Choose the repository under
                  {" "}
                  <span class="font-medium">Only select repositories</span>
                  . Nothing is written to GitHub until it is installed.
                </p>
                <Show when={current().install_url}>
                  {url => (
                    <a
                      href={url()}
                      target="_blank"
                      rel="noreferrer"
                      class="inline-block rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300"
                    >
                      Install on GitHub
                    </a>
                  )}
                </Show>
              </Match>
              <Match when={current().state === "installed"}>
                <p class={NOTE}>
                  {current().enabled
                    ? "Installed. error.menu writes a check on every scan."
                    : "Installed. Nothing is written until reporting is on."}
                </p>
              </Match>
            </Switch>
          </>
        )}
      </Show>
    </section>
  );
};
