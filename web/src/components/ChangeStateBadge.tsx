import type { JSX } from "@solidjs/web";
import { VsGitMerge, VsGitPullRequest, VsGitPullRequestClosed, VsGitPullRequestDraft } from "solid-icons/vs";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";

export type ChangeState = NonNullable<Analysis["forge"]["state"]>;

const BADGE = "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium";

export const CHANGE_STATE_TINT: Record<ChangeState, string> = {
  draft: "bg-stone-200 text-stone-700 dark:bg-stone-800 dark:text-stone-300",
  open: "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300",
  merged: "bg-violet-100 text-violet-800 dark:bg-violet-900/50 dark:text-violet-300",
  closed: "bg-slate-200 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
};

const BADGE_CLASS: Record<ChangeState, string> = {
  draft: `${BADGE} ${CHANGE_STATE_TINT.draft}`,
  open: `${BADGE} ${CHANGE_STATE_TINT.open}`,
  merged: `${BADGE} ${CHANGE_STATE_TINT.merged}`,
  closed: `${BADGE} ${CHANGE_STATE_TINT.closed}`,
};

export const CHANGE_STATE_ICONS: Record<ChangeState, () => JSX.Element> = {
  draft: () => <VsGitPullRequestDraft size={12} />,
  open: () => <VsGitPullRequest size={12} />,
  merged: () => <VsGitMerge size={12} />,
  closed: () => <VsGitPullRequestClosed size={12} />,
};

const LABELS: Record<ChangeState, string> = {
  draft: "Draft",
  open: "Open",
  merged: "Merged",
  closed: "Closed",
};

export const ChangeStateBadge = (properties: { state: ChangeState | undefined; }) => (
  <Show when={properties.state}>
    {state => (
      <span class={BADGE_CLASS[state()]}>
        {CHANGE_STATE_ICONS[state()]()}
        {LABELS[state()]}
      </span>
    )}
  </Show>
);
