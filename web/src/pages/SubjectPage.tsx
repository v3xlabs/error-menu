import { useParams, useSearchParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { FiArrowLeft, FiCheck, FiExternalLink } from "solid-icons/fi";
import { createEffect, createSignal, For, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { listAnalyses, readProject, scanChange } from "../api/projects";
import type { components } from "../api/schema.gen";
import { AnalyzerCard } from "../components/AnalyzerCard";
import { ChangeStateBadge } from "../components/ChangeStateBadge";
import { PersonAvatar } from "../components/PersonAvatar";
import { StatusDot, toneLabel } from "../components/StatusDot";
import type { AnalyzerRun } from "../domain/analysis";
import {
  analysisTone,
  distinctPeople,
  forgeKind,
  latestPerHead,
  mergedChangeFor,
  roleLabel,
  rolesOf,
  runTone,
  scanPath,
  subjectLabel,
} from "../domain/analysis";
import { analyzerByRunId } from "../domain/analyzer";

// Analyzers that ran clean and have nothing to show share one row. Set to false to give each
// its own card again.
const IS_COLLAPSE_QUIET_ANALYZERS = true;

const isQuiet = (run: AnalyzerRun): boolean =>
  runTone(run) === "clear" && run.finding_count === 0 && run.signals.length === 0 && run.detail === undefined;

type RunState = { phase: "ready"; } | { phase: "running"; } | { phase: "error"; message: string; };
type HistoryState = { phase: "loading"; } | { phase: "loaded"; analyses: readonly Analysis[]; nextBefore: string | undefined; } | { phase: "error"; message: string; };

export const SubjectPage = () => {
  const [historyState, setHistoryState] = createSignal<HistoryState>({ phase: "loading" });
  const [project, setProject] = createSignal<components["schemas"]["ProjectOutput"]>();
  const [projectError, setProjectError] = createSignal<string | null>(null);
  const [runState, setRunState] = createSignal<RunState>({ phase: "ready" });
  const [isLoadingMore, setIsLoadingMore] = createSignal(false);
  const [moreError, setMoreError] = createSignal<string | null>(null);
  const [mergedAnalyses, setMergedAnalyses] = createSignal<readonly Analysis[]>([]);
  const routeParameters = useParams<{ projectId: string; kind: string; key: string; }>();
  const [searchParameters] = useSearchParams<{ head: string; }>();
  let routeVersion = 0;
  let projectRead = 0;
  let historyRead = 0;
  let scanRead = 0;

  const isCurrent = (projectId: string, kind: string, key: string, version: number): boolean =>
    routeParameters.projectId === projectId
    && routeParameters.kind === kind
    && routeParameters.key === key
    && routeVersion === version;

  const loadHistory = async (projectId: string, kind: string, key: string): Promise<void> => {
    if (routeParameters.projectId !== projectId || routeParameters.kind !== kind || routeParameters.key !== key) return;

    const version = routeVersion;
    const request = ++historyRead;
    const analyses: Analysis[] = [];
    const head = searchParameters.head;
    let before: string | undefined;

    do {
      const result = await listAnalyses(projectId, { kind, key, ...(before !== undefined && { before }) });

      if (request !== historyRead || !isCurrent(projectId, kind, key, version)) return;

      if (!result.ok) {
        setHistoryState({ phase: "error", message: result.message });

        return;
      }

      analyses.push(...result.value.analyses);
      before = result.value.next_before;
    } while (head !== undefined && before !== undefined && analyses.every(analysis => analysis.head_sha !== head));

    setHistoryState({ phase: "loaded", analyses, nextBefore: before });
  };
  const loadMore = async (): Promise<void> => {
    const loaded = historyState();

    if (loaded.phase !== "loaded" || loaded.nextBefore === undefined || isLoadingMore()) return;

    const { projectId, kind, key } = routeParameters;
    const version = routeVersion;
    const request = ++historyRead;

    setMoreError(null);
    setIsLoadingMore(true);
    const result = await listAnalyses(projectId, { kind, key, before: loaded.nextBefore });

    if (request !== historyRead || !isCurrent(projectId, kind, key, version)) return;

    setIsLoadingMore(false);

    if (!result.ok) {
      setMoreError(result.message);

      return;
    }

    setHistoryState({ phase: "loaded", analyses: [...loaded.analyses, ...result.value.analyses], nextBefore: result.value.next_before });
  };
  const loadMergedChanges = async (projectId: string, kind: string, key: string): Promise<void> => {
    if (kind !== "branch") return;

    const version = routeVersion;
    const analyses: Analysis[] = [];
    let before: string | undefined;

    do {
      const result = await listAnalyses(projectId, { latest: true, ...(before !== undefined && { before }) });

      if (!isCurrent(projectId, kind, key, version) || !result.ok) return;

      analyses.push(...result.value.analyses);
      before = result.value.next_before;
    } while (before !== undefined);

    setMergedAnalyses(analyses);
  };

  const analyses = (): readonly Analysis[] => {
    const current = historyState();

    return current.phase === "loaded" ? current.analyses : [];
  };
  const hasMore = (): boolean => {
    const state = historyState();

    return state.phase === "loaded" && state.nextBefore !== undefined;
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

    return analysis === undefined ? undefined : mergedChangeFor(mergedAnalyses(), analysis.head_sha);
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

    const { projectId, kind, key } = routeParameters;

    if (analysis.subject.kind !== kind || analysis.subject.key !== key) return;

    const version = routeVersion;
    const request = ++scanRead;

    setRunState({ phase: "running" });

    const result = await scanChange(projectId, Number(analysis.subject.key));

    if (request !== scanRead || !isCurrent(projectId, kind, key, version)) return;

    if (!result.ok) {
      setRunState({ phase: "error", message: result.message });

      return;
    }

    setRunState({ phase: "ready" });
    await loadHistory(projectId, kind, key);
  };

  createEffect(
    () => [routeParameters.projectId, routeParameters.kind, routeParameters.key, searchParameters.head] as const,
    ([projectId, kind, key]) => {
      const version = ++routeVersion;
      const request = ++projectRead;

      setIsLoadingMore(false);
      setMoreError(null);
      setMergedAnalyses([]);
      setHistoryState({ phase: "loading" });
      setProject(undefined);
      setProjectError(null);
      setRunState({ phase: "ready" });
      void readProject(projectId).then((result) => {
        if (request !== projectRead || !isCurrent(projectId, kind, key, version)) return;

        setProjectError(result.ok ? null : result.message);

        if (result.ok) setProject(result.value);
      });
      void loadHistory(projectId, kind, key);
      void loadMergedChanges(projectId, kind, key);

      return () => {
        routeVersion += 1;
      };
    },
  );

  const renderSubject = (analysis: Analysis): JSX.Element => {
    const failedCheckRuns = analysis.check_runs.filter(
      check => check.status === "completed" && check.conclusion === "failure",
    );
    const quietRuns = IS_COLLAPSE_QUIET_ANALYZERS ? analysis.analyzers.filter(isQuiet) : [];
    const shownRuns = analysis.analyzers.filter(run => !quietRuns.includes(run));

    return (
      <div class="space-y-6">
        <div class="flex flex-wrap items-start justify-between gap-4">
          <div class="min-w-0 flex-1">
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
              <p class="flex max-w-md flex-wrap items-center gap-2 rounded-control bg-violet-50 px-4 py-2.5 text-sm text-violet-900 dark:bg-violet-950/40 dark:text-violet-200">
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
        </div>
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
              <For each={shownRuns}>
                {analyzer => <AnalyzerCard run={analyzer} />}
              </For>
              <Show when={quietRuns.length > 0}>
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1 p-4">
                  <span class="flex items-center gap-2 text-sm font-medium text-slate-900 dark:text-slate-100">
                    <span class="text-emerald-600 dark:text-emerald-400"><FiCheck size={14} /></span>
                    {`${quietRuns.length} ${quietRuns.length === 1 ? "analyzer" : "analyzers"} found nothing`}
                  </span>
                  <span class="text-xs text-slate-500 dark:text-slate-500">
                    {quietRuns.map(run => analyzerByRunId(run.analyzer)?.label ?? run.analyzer).join(", ")}
                  </span>
                </div>
              </Show>
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
        <Show when={hasMore()}>
          <button
            type="button"
            disabled={isLoadingMore()}
            onClick={() => void loadMore()}
            class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-300"
          >
            {isLoadingMore() ? "Loading..." : "Load older scans"}
          </button>
        </Show>
        <Show when={moreError()}>{message => <p role="alert" class="text-sm text-red-600 dark:text-red-400">{message()}</p>}</Show>
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

