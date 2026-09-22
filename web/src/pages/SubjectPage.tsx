import { useParams, useSearchParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { FiArrowLeft, FiExternalLink } from "solid-icons/fi";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { listAnalyses, readProject, scanChange } from "../api/projects";
import type { components } from "../api/schema.gen";
import { AnalyzerCard } from "../components/AnalyzerCard";
import { ChangeStateBadge } from "../components/ChangeStateBadge";
import { PersonAvatar } from "../components/PersonAvatar";
import { StatusDot, toneLabel } from "../components/StatusDot";
import {
  analysisTone,
  distinctPeople,
  forgeKind,
  latestPerHead,
  mergedChangeFor,
  roleLabel,
  rolesOf,
  scanPath,
  subjectLabel,
} from "../domain/analysis";

type RunState = { phase: "ready"; } | { phase: "running"; } | { phase: "error"; message: string; };
type HistoryState = { phase: "loading"; } | { phase: "loaded"; analyses: readonly Analysis[]; } | { phase: "error"; message: string; };

export const SubjectPage = () => {
  const [historyState, setHistoryState] = createSignal<HistoryState>({ phase: "loading" });
  const [project, setProject] = createSignal<components["schemas"]["ProjectOutput"]>();
  const [projectError, setProjectError] = createSignal<string | null>(null);
  const [runState, setRunState] = createSignal<RunState>({ phase: "ready" });
  const routeParameters = useParams<{ projectId: string; kind: string; key: string; }>();
  const [searchParameters] = useSearchParams<{ head: string; }>();

  const analyses = (): readonly Analysis[] => {
    const current = historyState();

    return current.phase === "loaded" ? current.analyses : [];
  };
  const history = (): readonly Analysis[] =>
    latestPerHead(
      analyses().filter(
        analysis => analysis.subject.kind === routeParameters.kind && analysis.subject.key === routeParameters.key,
      ),
    );
  // A reading is named by the commit it read. Without one, the subject's newest reading is
  // what a reader means.
  const shownIndex = (): number => {
    const head = searchParameters.head;

    return head === undefined ? 0 : history().findIndex(analysis => analysis.head_sha === head);
  };
  const current = (): Analysis | undefined => history()[shownIndex()];
  const earlierScans = (): readonly Analysis[] => history().slice(shownIndex() + 1);
  const mergedBy = (): Analysis | undefined => {
    const analysis = current();

    return analysis === undefined ? undefined : mergedChangeFor(analyses(), analysis.head_sha);
  };

  const canOperate = (): boolean => {
    const role = project()?.viewer_role;

    return role === "operator" || role === "owner";
  };

  const runError = (): string | null => {
    const current = runState();

    return current.phase === "error" ? current.message : null;
  };
  const runAnalyzers = async (analysis: Analysis): Promise<void> => {
    if (analysis.subject.kind !== "change") {
      setRunState({ phase: "error", message: "Only a pull request can be scanned this way." });

      return;
    }

    setRunState({ phase: "running" });

    const result = await scanChange(routeParameters.projectId, Number(analysis.subject.key));

    if (!result.ok) {
      setRunState({ phase: "error", message: result.message });

      return;
    }

    setRunState({ phase: "ready" });

    const refreshed = await listAnalyses(routeParameters.projectId);

    setHistoryState(refreshed.ok
      ? { phase: "loaded", analyses: refreshed.value }
      : { phase: "error", message: refreshed.message });
  };

  createEffect(
    () => routeParameters.projectId,
    (projectId) => {
      void readProject(projectId).then((result) => {
        setProjectError(result.ok ? null : result.message);

        if (result.ok) setProject(result.value);
      });
      void listAnalyses(projectId).then((result) => {
        setHistoryState(result.ok ? { phase: "loaded", analyses: result.value } : { phase: "error", message: result.message });
      });
    },
  );

  const renderSubject = (analysis: Analysis): JSX.Element => {
    const failedCheckRuns = analysis.check_runs.filter(
      check => check.status === "completed" && check.conclusion === "failure",
    );

    return (
      <div class="space-y-6">
        <div>
          <a href={`/projects/${routeParameters.projectId}`} class="inline-flex items-center gap-1 text-xs text-slate-500 hover:text-slate-700 dark:text-slate-500 dark:hover:text-slate-300">
            <FiArrowLeft size={12} />
            Back to project
          </a>
          <h1 class="mt-1 flex flex-wrap items-center gap-2 text-lg font-semibold">
            <ChangeStateBadge state={analysis.forge.state} />
            <Show when={analysis.forge.url} fallback={subjectLabel(analysis.subject)}>
              {url => (
                <a
                  href={url()}
                  target="_blank"
                  rel="noreferrer"
                  class="inline-flex items-center gap-1 hover:underline"
                >
                  {subjectLabel(analysis.subject)}
                  <span class="text-slate-400 dark:text-slate-500"><FiExternalLink size={12} /></span>
                </a>
              )}
            </Show>
            <Show when={analysis.forge.title}>
              {title => <span class="font-normal text-slate-600 dark:text-slate-300">{title()}</span>}
            </Show>
          </h1>
          <p class="mt-1 flex flex-wrap items-center gap-2 text-sm text-slate-500 dark:text-slate-500">
            <span>{toneLabel(analysisTone(analysis))}</span>
            <span class="font-mono text-xs">{analysis.head_sha.slice(0, 7)}</span>
            <Show when={analysis.forge.head_ref}>
              {headReference => (
                <span>
                  {headReference()}
                  <Show when={analysis.forge.base_ref}>
                    {baseReference => <>{` into ${baseReference()}`}</>}
                  </Show>
                </span>
              )}
            </Show>
          </p>
        </div>

        <Show when={mergedBy()}>
          {change => (
            <p class="flex flex-wrap items-center gap-2 rounded-control bg-violet-50 px-4 py-2.5 text-sm text-violet-900 dark:bg-violet-950/40 dark:text-violet-200">
              Merged by
              <ChangeStateBadge state={change().forge.state} />
              <a href={scanPath(routeParameters.projectId, change())} class="font-medium underline">
                {subjectLabel(change().subject)}
                <Show when={change().forge.title}>
                  {title => (
                    <>
                      {" "}
                      {title()}
                    </>
                  )}
                </Show>
              </a>
            </p>
          )}
        </Show>

        <section class="space-y-3">
          <h2 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">People</h2>
          <ul class="grid gap-2 sm:grid-cols-2">
            <For each={distinctPeople(analysis)} fallback={<li class="text-sm text-slate-500 dark:text-slate-500">No person is recorded for this scan.</li>}>
              {person => (
                <li class="flex items-center gap-3 rounded-panel bg-surface px-4 py-3">
                  <PersonAvatar
                    person={person}
                    analysis={analysis}
                    analyses={analyses()}
                    projectId={routeParameters.projectId}
                    projectForge={forgeKind(project()?.forge ?? "auto")}
                  />
                  <span class="min-w-0">
                    <span class="block truncate text-sm font-medium text-slate-900 dark:text-slate-100">{person.label}</span>
                    <span class="block truncate text-xs text-slate-500 dark:text-slate-400">
                      {rolesOf(analysis, person.identity).map(role => roleLabel(role))
                        .join(" - ")}
                    </span>
                  </span>
                </li>
              )}
            </For>
          </ul>
        </section>

        <section class="space-y-3">
          <h2 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">Analyzers</h2>
          <Show
            when={analysis.analyzers.length > 0}
            fallback={(
              <div class="rounded-panel bg-surface px-5 py-8 text-center">
                <p class="text-sm text-slate-500 dark:text-slate-500">
                  This change was recorded but never scanned. Closed and merged changes are kept for
                  their history rather than put through every analyzer.
                </p>
                <Show when={canOperate()}>
                  <button
                    type="button"
                    disabled={runState().phase === "running"}
                    onClick={() => void runAnalyzers(analysis)}
                    class="mt-3 rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
                  >
                    {runState().phase === "running" ? "Running..." : "Run analyzers now"}
                  </button>
                  <Show when={runError()}>
                    {message => <p class="mt-2 text-sm text-red-600 dark:text-red-400">{message()}</p>}
                  </Show>
                </Show>
              </div>
            )}
          >
            <div class="divide-y divide-hairline overflow-hidden rounded-panel bg-surface">
              <For each={analysis.analyzers}>
                {analyzer => <AnalyzerCard run={analyzer} />}
              </For>
            </div>
          </Show>
        </section>

        <Show when={failedCheckRuns.length > 0}>
          <section class="space-y-3">
            <h2 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">Failed CI checks</h2>
            <ul class="space-y-2">
              <For each={failedCheckRuns}>
                {check => (
                  <li class="flex flex-wrap items-center justify-between gap-2 rounded-control bg-red-50 px-4 py-2.5 text-sm dark:bg-red-950/40">
                    <span class="font-medium text-slate-900 dark:text-slate-100">{check.name}</span>
                    <span class="flex items-center gap-3 text-xs">
                      <Show when={check.url}>
                        {url => (
                          <a
                            href={url()}
                            target="_blank"
                            rel="noreferrer"
                            class="underline"
                          >
                            Details
                          </a>
                        )}
                      </Show>
                      <Show when={check.log_excerpt_ref}>
                        {reference => (
                          <a
                            href={reference()}
                            target="_blank"
                            rel="noreferrer"
                            class="underline"
                          >
                            Log reference
                          </a>
                        )}
                      </Show>
                    </span>
                  </li>
                )}
              </For>
            </ul>
          </section>
        </Show>

        <Show when={earlierScans().length > 0}>
          <section class="space-y-3">
            <h2 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">Earlier scans</h2>
            <ul class="space-y-2">
              <For each={earlierScans()}>
                {earlier => (
                  <li>
                    <a
                      href={scanPath(routeParameters.projectId, earlier)}
                      class="flex items-center justify-between gap-3 rounded-panel bg-surface px-4 py-3 text-sm hover:bg-raised"
                    >
                      <span class="flex items-center gap-2">
                        <StatusDot tone={analysisTone(earlier)} />
                        {toneLabel(analysisTone(earlier))}
                      </span>
                      <span class="font-mono text-xs text-slate-500 dark:text-slate-500">{earlier.head_sha.slice(0, 12)}</span>
                    </a>
                  </li>
                )}
              </For>
            </ul>
          </section>
        </Show>
      </div>
    );
  };

  const render = (): JSX.Element => {
    const state = historyState();

    if (state.phase === "loading") return <p class="text-sm text-slate-500 dark:text-slate-500">Loading...</p>;

    if (state.phase === "error") return <p class="text-sm text-red-600 dark:text-red-400">{state.message}</p>;

    const analysis = current();

    if (analysis === undefined) {
      return (
        <p class="text-sm text-slate-500 dark:text-slate-500">
          {searchParameters.head === undefined
            ? "Nothing has been scanned for this subject yet."
            : "No scan of that commit is recorded."}
        </p>
      );
    }

    return renderSubject(analysis);
  };

  return (
    <>
      <Show when={projectError()}>
        {message => <p class="mb-4 text-sm text-red-600 dark:text-red-400">{message()}</p>}
      </Show>
      {render()}
    </>
  );
};

