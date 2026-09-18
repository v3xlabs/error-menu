import { createMemo, createSignal, For, Show } from "solid-js";

import type { AnalyzerRun, PackageFinding } from "../domain/analysis";
import { runTone } from "../domain/analysis";
import { analyzerByRunId } from "../domain/analyzer";
import type { DependencyKind } from "../domain/dependency";
import { dependencyFiles, dependencyTotals } from "../domain/dependency";
import { severityCounts, severityFiles } from "../domain/severity";
import { signalFromOutput } from "../domain/signal";
import type { CountSlot } from "./Counts";
import { CountBar, CountRow, movementSlots, severitySlots } from "./Counts";
import { FileIcon } from "./FileIcon";
import { FindingLine } from "./FindingLine";
import { Gauge } from "./Gauge";
import { SignalBadge } from "./SignalBadge";

// A file with more findings than this is read by opening it, not by scrolling past it.
const SHOWN_FINDINGS = 10;

type RunFile = {
  path: string;
  directory: string;
  name: string;
  kind: DependencyKind;
  total: number;
  slots: readonly CountSlot[];
  findings: readonly PackageFinding[];
};

type Breakdown = { files: readonly RunFile[]; totals: readonly CountSlot[]; };

const FileRow = (properties: { file: RunFile; }) => {
  const [isOpen, setIsOpen] = createSignal(false);
  const shown = (): readonly PackageFinding[] =>
    (isOpen() ? properties.file.findings : properties.file.findings.slice(0, SHOWN_FINDINGS));
  const hidden = (): number | undefined => {
    const remaining = properties.file.findings.length - SHOWN_FINDINGS;

    return remaining > 0 ? remaining : undefined;
  };

  return (
    <li>
      <div class="flex items-center gap-3 px-3 py-1.5">
        <span class="flex shrink-0">
          <FileIcon kind={properties.file.kind} size={15} />
        </span>
        <span class="min-w-0 flex-1 truncate font-mono text-xs">
          <span class="text-slate-400 dark:text-slate-500">{properties.file.directory}</span>
          <span class="text-slate-900 dark:text-slate-100">{properties.file.name}</span>
        </span>
        <CountRow slots={properties.file.slots} />
        <CountBar slots={properties.file.slots} total={properties.file.total} />
      </div>
      <ul class="space-y-0.5 px-3 pb-2 pl-9">
        <For each={shown()}>{finding => <FindingLine finding={finding} />}</For>
        <Show when={hidden()}>
          {count => (
            <li>
              <button
                type="button"
                onClick={() => setIsOpen(!isOpen())}
                class="text-xs text-slate-500 underline hover:text-slate-800 dark:text-slate-400 dark:hover:text-slate-200"
              >
                {isOpen() ? "Show fewer" : `Show ${count()} more`}
              </button>
            </li>
          )}
        </Show>
      </ul>
    </li>
  );
};

export const AnalyzerCard = (properties: { run: AnalyzerRun; }) => {
  const analyzer = () => analyzerByRunId(properties.run.analyzer);

  // Grouping walks every finding, and one lockfile run can carry ten thousand of them.
  const breakdown = createMemo<Breakdown>(() => {
    const findings = properties.run.findings;
    const byPackage = findings.length > 0 && findings.every(finding => finding.package !== undefined);

    if (byPackage) {
      const files = dependencyFiles(findings);

      return {
        files: files.map(file => ({
          path: file.path,
          directory: file.directory,
          name: file.name,
          kind: file.kind,
          total: file.total,
          slots: movementSlots(file.counts),
          findings: findings.filter(finding => finding.path === file.path),
        })),
        totals: movementSlots(dependencyTotals(files)),
      };
    }

    return {
      files: severityFiles(findings).map(file => ({
        path: file.path,
        directory: file.directory,
        name: file.name,
        kind: file.kind,
        total: file.total,
        slots: severitySlots(file.counts),
        findings: file.findings,
      })),
      totals: severitySlots(severityCounts(findings)),
    };
  });

  const signals = () => properties.run.signals.flatMap((signal) => {
    const normalized = signalFromOutput(signal);

    return normalized === undefined ? [] : [normalized];
  });

  return (
    <div class="p-3">
      <div class="min-w-0 space-y-2">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <span class="flex items-center gap-2">
            <Show when={analyzer()}>
              {known => (
                <span class="flex text-slate-500 dark:text-slate-400">
                  {known().icon({ size: 14 })}
                </span>
              )}
            </Show>
            <span class="text-sm font-medium text-slate-900 dark:text-slate-100">
              {analyzer()?.label ?? properties.run.analyzer}
            </span>
          </span>
          <span class="flex items-center gap-3">
            <Show when={properties.run.finding_count > 0}>
              <CountRow slots={breakdown().totals} />
              <CountBar slots={breakdown().totals} total={properties.run.finding_count} />
            </Show>
            <Gauge
              tone={runTone(properties.run)}
              value={properties.run.finding_count}
              label={`${analyzer()?.label ?? properties.run.analyzer} ${properties.run.status}, ${properties.run.finding_count} ${properties.run.finding_count === 1 ? "finding" : "findings"}`}
            />
          </span>
        </div>
        <Show when={properties.run.detail}>
          {detail => <p class="text-xs text-red-600 dark:text-red-400">{detail()}</p>}
        </Show>
        <Show when={breakdown().files.length > 0}>
          <ul class="divide-y divide-slate-200 rounded-md border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            <For each={breakdown().files}>{file => <FileRow file={file} />}</For>
          </ul>
        </Show>
        <Show when={signals().length > 0}>
          <div class="grid gap-2 sm:grid-cols-2">
            <For each={signals()}>{signal => <SignalBadge signal={signal} />}</For>
          </div>
        </Show>
      </div>
    </div>
  );
};
