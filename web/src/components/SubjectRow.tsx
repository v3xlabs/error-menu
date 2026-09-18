import { Tooltip } from "@kobalte/core/tooltip";
import { FiHash } from "solid-icons/fi";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";
import type { ForgeKind } from "../domain/analysis";
import { distinctPeople, findingsOf, subjectLabel, subjectPath, worstFindingTone } from "../domain/analysis";
import { ChangeStateBadge } from "./ChangeStateBadge";
import { Gauge } from "./Gauge";
import { PersonRow } from "./PersonAvatar";
import { SignatureBadge } from "./SignatureBadge";

export const SubjectRow = (properties: {
  projectForge: ForgeKind | undefined;
  analysis: Analysis;
  analyses: readonly Analysis[];
  projectId: string;
}) => {
  const findingCount = (): number => findingsOf(properties.analysis).length;

  return (
    <div class="rounded-md border border-slate-200 px-3 py-2 dark:border-slate-800">
      <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
        <span class="flex min-w-0 items-center gap-2">
          <ChangeStateBadge state={properties.analysis.forge.state} />
          <a
            href={subjectPath(properties.projectId, properties.analysis)}
            class="truncate text-sm font-medium text-slate-900 hover:underline dark:text-slate-100"
          >
            {subjectLabel(properties.analysis.subject)}
            <Show when={properties.analysis.forge.title}>
              {title => <span class="ml-2 font-normal text-slate-600 dark:text-slate-300">{title()}</span>}
            </Show>
          </a>
        </span>
        <span class="flex items-center gap-2">
          <Show when={findingCount() > 0}>
            <Gauge
              tone={worstFindingTone(properties.analysis)}
              value={findingCount()}
              label={`${findingCount()} findings across ${properties.analysis.analyzers.length} analyzers`}
            />
          </Show>
          <Show when={findingCount() === 0 && properties.analysis.analyzers.length > 0}>
            <Gauge tone="clear" value={0} label="Every analyzer finished and found nothing" />
          </Show>
        </span>
      </div>
      <div class="mt-1.5 flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
        <PersonRow
          people={distinctPeople(properties.analysis)}
          analysis={properties.analysis}
          analyses={properties.analyses}
          projectId={properties.projectId}
          projectForge={properties.projectForge}
        />
        <span class="flex items-center gap-2">
          <SignatureBadge signature={properties.analysis.signature} />
          <Tooltip>
            <Tooltip.Trigger as="span" class="inline-flex items-center gap-0.5 text-xs text-slate-400 dark:text-slate-500">
              <FiHash size={12} />
              {properties.analysis.head_sha.slice(0, 7)}
            </Tooltip.Trigger>
            <Tooltip.Portal>
              <Tooltip.Content class="z-50 rounded-md bg-slate-900 px-2.5 py-1.5 font-mono text-xs text-white dark:bg-slate-100 dark:text-slate-900">
                <Tooltip.Arrow />
                {properties.analysis.head_sha}
              </Tooltip.Content>
            </Tooltip.Portal>
          </Tooltip>
        </span>
      </div>
    </div>
  );
};
