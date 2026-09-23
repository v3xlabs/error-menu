import { Show } from "solid-js";

import type { components } from "../api/schema.gen";
import type { QueueState } from "../domain/job";
import { InspectModal } from "./InspectModal";
import { ProjectMark } from "./ProjectMark";
import { ProjectSettingsModal } from "./ProjectSettingsModal";
import { QueueStateBadge } from "./QueueState";

type Project = components["schemas"]["ProjectOutput"];

export const ProjectHeader = (properties: {
  project: Project;
  isDiscovering: boolean;
  queue: QueueState;
  onDiscover: () => void;
  onAnalysed: () => void;
  onSaved: () => void;
}) => {
  const canOperate = (): boolean =>
    properties.project.viewer_role === "operator" || properties.project.viewer_role === "owner";
  const isOwner = (): boolean => properties.project.viewer_role === "owner";

  return (
    <header class="flex items-start justify-between gap-6">
      <div class="flex min-w-0 flex-1 items-start gap-3">
        <ProjectMark project={properties.project} size={40} />
        <div class="min-w-0">
          <div class="flex flex-wrap items-baseline gap-x-2">
            <h1 class="text-lg font-semibold">{properties.project.name}</h1>
            <a
              href={`/orgs/${properties.project.organization_id}`}
              class="text-sm text-slate-500 hover:underline dark:text-slate-400"
            >
              {properties.project.organization_name}
            </a>
          </div>
          <p class="truncate text-sm text-slate-500 dark:text-slate-500">{properties.project.remote_url}</p>
          <Show when={properties.project.description}>
            {description => <p class="mt-2 text-sm text-slate-600 dark:text-slate-300">{description()}</p>}
          </Show>
        </div>
      </div>
      <div class="flex shrink-0 flex-col items-end gap-2">
        <div class="flex items-center gap-2">
          <Show when={canOperate()}>
            <InspectModal projectId={properties.project.project_id} onAnalysed={() => properties.onAnalysed()} />
            <button
              type="button"
              disabled={properties.isDiscovering}
              onClick={() => properties.onDiscover()}
              class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
            >
              {properties.isDiscovering ? "Discovering..." : "Discover changes"}
            </button>
          </Show>
          <Show when={isOwner()}>
            <ProjectSettingsModal project={properties.project} onSaved={() => properties.onSaved()} />
          </Show>
        </div>
        <QueueStateBadge state={properties.queue} time="relative" />
      </div>
    </header>
  );
};
