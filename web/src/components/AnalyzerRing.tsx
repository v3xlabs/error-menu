import { Tooltip } from "@kobalte/core/tooltip";
import { For, Match, Show, Switch } from "solid-js";

import type { StatusTone } from "../domain/analysis";
import { analyzerByRunId, ANALYZERS } from "../domain/analyzer";
import { TONE_STROKE } from "./Gauge";
import { StatusDot } from "./StatusDot";

export type AnalyzerRingEntry = {
  analyzer: string;
  tone: StatusTone;
  finding_count: number;
};

// The ring is drawn in a fixed 40 unit box and scaled by the `size` prop, so the sector
// width, the gap and the centre glyph keep their proportions at every size.
const BOX = 40;
const CENTRE = BOX / 2;
const STROKE = 4.5;
const RADIUS = CENTRE - STROKE / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
const GAP = 2.5;

const seat = (runId: string): number => {
  const known = ANALYZERS.findIndex(analyzer => analyzer.runId === runId);

  return known === -1 ? ANALYZERS.length : known;
};

// An analyzer the client does not know yet still gets a sector, because the backend owns
// the list and can ship one before this build does. It sits after the known seats so that
// the known analyzers keep the same position on every project.
const seated = (entries: readonly AnalyzerRingEntry[]): readonly AnalyzerRingEntry[] =>
  entries.toSorted((left, right) => seat(left.analyzer) - seat(right.analyzer));

const analyzerLabel = (runId: string): string => analyzerByRunId(runId)?.label ?? runId;

const findingWord = (count: number): string => (count === 1 ? "finding" : "findings");

const summary = (entries: readonly AnalyzerRingEntry[], findings: number): string => {
  if (entries.length === 0) return "No analyzer has run";

  const quiet = entries.filter(entry => entry.tone === "clear").length;
  const running = entries.filter(entry => entry.tone === "running").length;
  const parts = [`${quiet} of ${entries.length} analyzers found nothing to flag`];

  if (running > 0) parts.push(`${running} running`);

  parts.push(`${findings} ${findingWord(findings)}`);

  return parts.join(", ");
};

export const AnalyzerRing = (properties: { entries: readonly AnalyzerRingEntry[]; size?: number; }) => {
  const entries = (): readonly AnalyzerRingEntry[] => seated(properties.entries);
  const findings = (): number => entries().reduce((sum, entry) => sum + entry.finding_count, 0);
  const isAllClear = (): boolean => entries().length > 0 && entries().every(entry => entry.tone === "clear");
  const sector = (): number => CIRCUMFERENCE / entries().length;

  return (
    <Tooltip>
      <Tooltip.Trigger as="span" class="inline-flex shrink-0 items-center">
        <svg
          role="img"
          aria-label={summary(entries(), findings())}
          width={properties.size ?? 36}
          height={properties.size ?? 36}
          viewBox={`0 0 ${BOX} ${BOX}`}
        >
          <g fill="none" stroke-width={STROKE} transform={`rotate(-90 ${CENTRE} ${CENTRE})`}>
            <Show when={entries().length === 0}>
              <circle
                cx={CENTRE}
                cy={CENTRE}
                r={RADIUS}
                class={TONE_STROKE.unscanned}
              />
            </Show>
            <For each={entries()}>
              {(entry, index) => (
                <circle
                  cx={CENTRE}
                  cy={CENTRE}
                  r={RADIUS}
                  class={TONE_STROKE[entry.tone]}
                  stroke-dasharray={`${sector() - GAP} ${CIRCUMFERENCE - sector() + GAP}`}
                  stroke-dashoffset={-(index() * sector() + GAP / 2)}
                />
              )}
            </For>
          </g>
          <Switch
            fallback={(
              <line
                x1={CENTRE - 3.5}
                y1={CENTRE}
                x2={CENTRE + 3.5}
                y2={CENTRE}
                class="stroke-slate-400 dark:stroke-slate-500"
                stroke-width="2"
                stroke-linecap="round"
              />
            )}
          >
            <Match when={findings() > 0}>
              <text
                x={CENTRE}
                y={CENTRE}
                text-anchor="middle"
                dominant-baseline="central"
                font-size="13"
                class="fill-slate-900 font-semibold tabular-nums dark:fill-slate-100"
              >
                {findings()}
              </text>
            </Match>
            <Match when={isAllClear()}>
              <path
                d={`M ${CENTRE - 4.5} ${CENTRE + 0.2} l 3.4 3.4 l 6.1 -6.7`}
                class="stroke-emerald-600 dark:stroke-emerald-400"
                fill="none"
                stroke-width="2.4"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </Match>
          </Switch>
        </svg>
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content class="z-50 rounded-md bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900">
          <Tooltip.Arrow />
          <Show when={entries().length > 0} fallback={<span>No analyzer has run.</span>}>
            <ul class="space-y-1">
              <For each={entries()}>
                {entry => (
                  <li class="flex items-center gap-2">
                    <StatusDot tone={entry.tone} />
                    <span class="flex-1">{analyzerLabel(entry.analyzer)}</span>
                    <span class="tabular-nums">{entry.finding_count}</span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip>
  );
};
