import { Tooltip } from "@kobalte/core/tooltip";
import { For, Show } from "solid-js";

import type { Analysis, Commit } from "../api/projects";
import { analysisTone, runTone, scanPath } from "../domain/analysis";
import { sinceNow } from "../domain/time";
import type { AnalyzerRingEntry } from "./AnalyzerRing";
import { AnalyzerRing } from "./AnalyzerRing";
import { StatusDot } from "./StatusDot";

const ringEntries = (analysis: Analysis): readonly AnalyzerRingEntry[] =>
  analysis.analyzers.map(run => ({
    analyzer: run.analyzer,
    tone: runTone(run),
    finding_count: run.findings.length,
  }));

const CommitRow = (properties: { commit: Commit; scan: Analysis | undefined; projectId: string; }) => (
  <li class="flex gap-3 border-b border-hairline py-3">
    <span class="mt-1.5 shrink-0">
      <Show
        when={properties.scan}
        fallback={<span class="block size-2.5 rounded-full border border-slate-400 dark:border-slate-600" />}
      >
        {scan => <StatusDot tone={analysisTone(scan())} />}
      </Show>
    </span>
    <span class="min-w-0 flex-1">
      <Show
        when={properties.scan}
        fallback={<span class="block truncate text-sm text-slate-700 dark:text-slate-300">{properties.commit.summary}</span>}
      >
        {scan => (
          <a
            href={scanPath(properties.projectId, scan())}
            class="block truncate text-sm font-medium text-slate-900 hover:underline dark:text-slate-100"
          >
            {properties.commit.summary}
          </a>
        )}
      </Show>
      <span class="flex flex-wrap items-center gap-x-2 text-xs text-slate-500 dark:text-slate-500">
        <Tooltip>
          <Tooltip.Trigger as="span" class="font-mono">{properties.commit.sha.slice(0, 7)}</Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content class="z-50 rounded-control bg-slate-900 px-2.5 py-1.5 font-mono text-xs text-white dark:bg-slate-100 dark:text-slate-900">
              <Tooltip.Arrow />
              {properties.commit.sha}
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip>
        <Show when={properties.commit.author}>{author => <span class="truncate">{author()}</span>}</Show>
        <span>{sinceNow(properties.commit.authored_at)}</span>
      </span>
    </span>
    <Show when={properties.scan}>
      {scan => (
        <span class="shrink-0">
          <AnalyzerRing entries={ringEntries(scan())} size={28} />
        </span>
      )}
    </Show>
  </li>
);

// A commit error.menu never scanned still belongs on the line, because the gap between
// two scanned commits is what a reader needs to see.
export const CommitHistory = (properties: {
  projectId: string;
  commits: readonly Commit[];
  analyses: readonly Analysis[];
}) => {
  const scanOf = (sha: string): Analysis | undefined =>
    properties.analyses.find(analysis => analysis.head_sha === sha);

  return (
    <ol>
      <For
        each={properties.commits}
        fallback={<li class="text-sm text-slate-500 dark:text-slate-500">No commit is readable yet.</li>}
      >
        {commit => (
          <CommitRow
            commit={commit}
            scan={scanOf(commit.sha)}
            projectId={properties.projectId}
          />
        )}
      </For>
    </ol>
  );
};
