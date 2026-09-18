import { Select } from "@kobalte/core/select";
import { Tabs } from "@kobalte/core/tabs";
import { Tooltip } from "@kobalte/core/tooltip";
import type { JSX } from "@solidjs/web";
import { FiCheck, FiChevronDown } from "solid-icons/fi";
import { createMemo, createSignal, For, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import type { ForgeKind, StatusTone } from "../domain/analysis";
import { analysisTone, byReviewOrder, findingsOf, latestPerSubject, scanned, subjectLabel, subjectPath, worstTone } from "../domain/analysis";
import type { DependencyFile } from "../domain/dependency";
import { dependencyFiles, dependencyTotals } from "../domain/dependency";
import type { ChangeFilter } from "./ChangeList";
import { CHANGE_FILTERS, ChangeList, isInFilter, OPEN_CHANGE_FILTER } from "./ChangeList";
import { ChangeStateBadge } from "./ChangeStateBadge";
import { CountRow, movementSlots } from "./Counts";
import { DependencyFileList } from "./DependencyFiles";
import { StatusDot, toneLabel } from "./StatusDot";
import { SubjectRow } from "./SubjectRow";

export type ProjectDashboardProperties = {
  projectId: string;
  projectForge: ForgeKind | undefined;
  analyses: readonly Analysis[];
  isDiscovering: boolean;
  discoverError: string | null;
  onDiscover: () => void;
  onAnalysed: () => void;
};

const TAB_TRIGGER = "-mb-px border-b-2 border-transparent px-1 pb-2 text-sm font-medium text-slate-500 hover:text-slate-800 data-[selected]:border-slate-900 data-[selected]:text-slate-900 dark:text-slate-400 dark:hover:text-slate-200 dark:data-[selected]:border-slate-100 dark:data-[selected]:text-slate-100";

const ToneValue = (properties: { tone: StatusTone | null; }) => (
  <Show when={properties.tone} fallback={<span class="text-slate-500 dark:text-slate-500">No data</span>}>
    {tone => (
      <>
        <StatusDot tone={tone()} />
        {toneLabel(tone())}
      </>
    )}
  </Show>
);

const FILTER_COUNT = "ml-auto pl-3 tabular-nums opacity-70";

const DependencyRow = (properties: { analysis: Analysis; files: readonly DependencyFile[]; projectId: string; }) => (
  <li class="space-y-2">
    <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
      <span class="flex min-w-0 items-center gap-2">
        <ChangeStateBadge state={properties.analysis.forge.state} />
        <a
          href={subjectPath(properties.projectId, properties.analysis)}
          class="text-sm font-medium text-slate-900 hover:underline dark:text-slate-100"
        >
          {subjectLabel(properties.analysis.subject)}
          <Show when={properties.analysis.forge.title}>
            {title => <span class="ml-2 font-normal text-slate-600 dark:text-slate-300">{title()}</span>}
          </Show>
        </a>
      </span>
      <span class="flex items-center gap-3">
        <span class="text-xs text-slate-500 dark:text-slate-400">
          {properties.files.length === 1 ? "1 file" : `${properties.files.length} files`}
        </span>
        <CountRow slots={movementSlots(dependencyTotals(properties.files))} />
      </span>
    </div>
    <DependencyFileList files={properties.files} />
  </li>
);

const empty = (message: string): JSX.Element => (
  <p class="rounded-lg border border-dashed border-slate-300 px-4 py-6 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">
    {message}
  </p>
);

export const ProjectDashboard = (properties: ProjectDashboardProperties) => {
  const [selectedFilter, setSelectedFilter] = createSignal(OPEN_CHANGE_FILTER);
  const filter = (): ChangeFilter => selectedFilter().value;
  const latest = (): readonly Analysis[] => latestPerSubject(properties.analyses);
  const branches = (): readonly Analysis[] => latest().filter(analysis => analysis.subject.kind === "branch");
  const changes = (): readonly Analysis[] => byReviewOrder(latest().filter(analysis => analysis.subject.kind === "change"));
  const openChanges = (): number => changes().filter(analysis => analysis.forge.state === "open").length;
  // Grouping walks every finding of every subject, and one pnpm lockfile can carry ten
  // thousand, so it runs once per analyses change rather than once per read.
  const withDependencies = createMemo(() =>
    latest()
      .map(analysis => ({ analysis, files: dependencyFiles(findingsOf(analysis)) }))
      .filter(entry => entry.files.length > 0));
  const openExcerpt = (): readonly Analysis[] => changes().filter(analysis => analysis.forge.state === "open")
    .slice(0, 4);
  const filteredChanges = (): readonly Analysis[] => changes().filter(analysis => isInFilter(analysis, filter()));
  const countFor = (option: ChangeFilter): number => changes().filter(analysis => isInFilter(analysis, option)).length;
  const defaultBranchLabel = (): string => {
    const branch = branches()[0];

    return branch === undefined ? "Not discovered yet" : subjectLabel(branch.subject);
  };

  return (
    <div class="space-y-6">
      <Show when={properties.discoverError}>
        {message => <p class="text-sm text-red-600 dark:text-red-400">{message()}</p>}
      </Show>
      <div class="grid divide-y divide-slate-200 rounded-lg border border-slate-200 sm:grid-cols-2 sm:divide-x sm:divide-y-0 dark:divide-slate-800 dark:border-slate-800">
        <div class="p-4">
          <p class="text-xs font-medium tracking-wide text-slate-500 uppercase dark:text-slate-400">Default branch</p>
          <p class="mt-2 flex items-center gap-2 text-lg font-semibold text-slate-900 dark:text-slate-100">
            <ToneValue tone={worstTone(branches().map(analysis => analysisTone(analysis)))} />
          </p>
          <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">{defaultBranchLabel()}</p>
        </div>
        <div class="p-4">
          <p class="text-xs font-medium tracking-wide text-slate-500 uppercase dark:text-slate-400">Open pull requests</p>
          <p class="mt-2 text-lg font-semibold text-slate-900 dark:text-slate-100">{openChanges()}</p>
          <ul class="mt-1 space-y-0.5">
            <For
              each={openExcerpt()}
              fallback={<li class="text-sm text-slate-500 dark:text-slate-400">Nothing is open right now.</li>}
            >
              {analysis => (
                <li class="flex items-center gap-1.5 truncate text-sm text-slate-600 dark:text-slate-300">
                  <StatusDot tone={analysisTone(analysis)} />
                  <a href={subjectPath(properties.projectId, analysis)} class="truncate hover:underline">
                    {subjectLabel(analysis.subject)}
                    <Show when={analysis.forge.title}>{title => <span>{` ${title()}`}</span>}</Show>
                  </a>
                </li>
              )}
            </For>
          </ul>
        </div>
      </div>

      <Tabs>
        <Tabs.List class="flex gap-6 border-b border-slate-200 dark:border-slate-800">
          <Tabs.Trigger value="timeline" class={TAB_TRIGGER}>Timeline</Tabs.Trigger>
          <Tabs.Trigger value="pull-requests" class={TAB_TRIGGER}>
            Pull requests
            <Show when={openChanges() > 0}>
              <span class="ml-2 rounded-full bg-slate-100 px-2 py-0.5 text-xs tabular-nums dark:bg-slate-800">{openChanges()}</span>
            </Show>
          </Tabs.Trigger>
          <Tabs.Trigger value="dependencies" class={TAB_TRIGGER}>Dependencies</Tabs.Trigger>
        </Tabs.List>

        <Tabs.Content value="timeline" class="pt-4">
          <div class="space-y-6">
            <section class="space-y-3">
              <h3 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">Default branch</h3>
              <ul class="space-y-2">
                <For each={branches()} fallback={empty("Run discovery to read the default branch.")}>
                  {analysis => (
                    <SubjectRow
                      analysis={analysis}
                      analyses={properties.analyses}
                      projectId={properties.projectId}
                      projectForge={properties.projectForge}
                    />
                  )}
                </For>
              </ul>
            </section>
            <section class="space-y-3">
              <h3 class="text-sm font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">Recent scans</h3>
              <ul class="space-y-2">
                <For each={scanned(properties.analyses)} fallback={empty("No scan has run for this project.")}>
                  {analysis => (
                    <SubjectRow
                      analysis={analysis}
                      analyses={properties.analyses}
                      projectId={properties.projectId}
                      projectForge={properties.projectForge}
                    />
                  )}
                </For>
              </ul>
            </section>
          </div>
        </Tabs.Content>

        <Tabs.Content value="pull-requests" class="space-y-3 pt-4">
          <Select
            options={CHANGE_FILTERS}
            optionValue="value"
            optionTextValue="label"
            value={selectedFilter()}
            onChange={option => setSelectedFilter(option ?? OPEN_CHANGE_FILTER)}
            itemComponent={item => (
              <Select.Item item={item.item} class={item.item.rawValue.pill}>
                {item.item.rawValue.icon()}
                <Select.ItemLabel>{item.item.rawValue.label}</Select.ItemLabel>
                <span class={FILTER_COUNT}>{countFor(item.item.rawValue.value)}</span>
                <Select.ItemIndicator>
                  <FiCheck size={14} />
                </Select.ItemIndicator>
              </Select.Item>
            )}
          >
            <Select.Trigger class={selectedFilter().pill} aria-label="Filter pull requests">
              {selectedFilter().icon()}
              <span>{selectedFilter().label}</span>
              <span class="tabular-nums opacity-70">{countFor(filter())}</span>
              <Select.Icon>
                <FiChevronDown size={14} />
              </Select.Icon>
            </Select.Trigger>
            <Select.Portal>
              <Select.Content class="z-50 min-w-52 rounded-md border border-slate-200 bg-white p-1 shadow-lg dark:border-slate-800 dark:bg-slate-900">
                <Select.Listbox class="space-y-1" />
              </Select.Content>
            </Select.Portal>
          </Select>
          <ChangeList
            analyses={filteredChanges()}
            all={properties.analyses}
            projectId={properties.projectId}
            projectForge={properties.projectForge}
          />
        </Tabs.Content>

        <Tabs.Content value="dependencies" class="pt-4">
          <div class="space-y-3">
            <div class="flex justify-end">
              <Tooltip>
                <Tooltip.Trigger
                  type="button"
                  aria-disabled="true"
                  class="cursor-not-allowed rounded-md border border-slate-300 px-2.5 py-1 text-xs font-medium text-slate-500 opacity-70 dark:border-slate-700 dark:text-slate-400"
                >
                  Upgrade recommendations
                </Tooltip.Trigger>
                <Tooltip.Portal>
                  <Tooltip.Content class="z-50 rounded-md bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900">
                    <Tooltip.Arrow />
                    error.menu reads no package registry, so it cannot recommend an upgrade yet.
                  </Tooltip.Content>
                </Tooltip.Portal>
              </Tooltip>
            </div>
            <ul class="space-y-4">
              <For each={withDependencies()} fallback={empty("No scanned change touched a lockfile.")}>
                {entry => (
                  <DependencyRow
                    analysis={entry.analysis}
                    files={entry.files}
                    projectId={properties.projectId}
                  />
                )}
              </For>
            </ul>
          </div>
        </Tabs.Content>
      </Tabs>
    </div>
  );
};
