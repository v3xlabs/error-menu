import type { JSX } from "@solidjs/web";
import type { VirtualItem } from "@tanstack/virtual-core";
import { elementScroll, observeElementOffset, observeElementRect, Virtualizer } from "@tanstack/virtual-core";
import { FiList } from "solid-icons/fi";
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import type { ForgeKind } from "../domain/analysis";
import { CHANGE_STATE_ICONS, CHANGE_STATE_TINT } from "./ChangeStateBadge";
import { TONE_ICONS, TONE_TINT } from "./Gauge";
import { SubjectRow } from "./SubjectRow";

export type ChangeFilter = "open" | "merged" | "closed" | "unscanned" | "flagged" | "all";

export type ChangeFilterOption = { value: ChangeFilter; label: string; pill: string; icon: () => JSX.Element; };

const PILL = "flex items-center gap-2 rounded-full py-1 pr-2.5 pl-3 text-xs font-medium data-[highlighted]:ring-1 data-[highlighted]:ring-slate-400 dark:data-[highlighted]:ring-slate-500";

export const OPEN_CHANGE_FILTER: ChangeFilterOption = {
  value: "open",
  label: "Open",
  pill: `${PILL} ${CHANGE_STATE_TINT.open}`,
  icon: CHANGE_STATE_ICONS.open,
};

// Mutable because the select primitive takes its options as a mutable array.
export const CHANGE_FILTERS: ChangeFilterOption[] = [
  OPEN_CHANGE_FILTER,
  { value: "merged", label: "Merged", pill: `${PILL} ${CHANGE_STATE_TINT.merged}`, icon: CHANGE_STATE_ICONS.merged },
  { value: "closed", label: "Closed", pill: `${PILL} ${CHANGE_STATE_TINT.closed}`, icon: CHANGE_STATE_ICONS.closed },
  { value: "unscanned", label: "Not scanned", pill: `${PILL} ${TONE_TINT.unscanned}`, icon: TONE_ICONS.unscanned },
  { value: "flagged", label: "Needs attention", pill: `${PILL} ${TONE_TINT.attention}`, icon: TONE_ICONS.attention },
  { value: "all", label: "All", pill: `${PILL} ${TONE_TINT.unscanned}`, icon: () => <FiList size={12} /> },
];

export const isInFilter = (analysis: Analysis, filter: ChangeFilter): boolean => {
  switch (filter) {
    case "all": {
      return true;
    }
    case "unscanned": {
      return analysis.analyzers.length === 0;
    }
    case "flagged": {
      return analysis.status === "alarming" || analysis.status === "attention";
    }
    default: {
      return analysis.forge.state === filter;
    }
  }
};

// A repository with a thousand changes should cost the same to draw as one with ten, so
// only the rows in view are built. The height is an estimate; the virtualizer measures the
// real rows as they mount.
const ESTIMATED_ROW_HEIGHT = 78;
const VISIBLE_HEIGHT = 620;
const OVERSCAN = 6;

export const ChangeList = (properties: {
  analyses: readonly Analysis[];
  all: readonly Analysis[];
  projectId: string;
  projectForge: ForgeKind | undefined;
}) => {
  const [scroller, setScroller] = createSignal<HTMLDivElement>();
  const [rows, setRows] = createSignal<readonly VirtualItem[]>([]);
  const [totalHeight, setTotalHeight] = createSignal(0);
  const [virtualizer, setVirtualizer] = createSignal<Virtualizer<HTMLDivElement, Element>>();

  // Changing the filter changes how many rows exist, and the count is fixed when the
  // virtualizer is built, so it is rebuilt rather than mutated. That also returns the
  // reader to the top, which is what picking a different filter should do.
  createEffect(
    () => [scroller(), properties.analyses.length] as const,
    ([element, count]) => {
      if (element === undefined) return;

      const instance = new Virtualizer<HTMLDivElement, Element>({
        count,
        getScrollElement: () => element,
        estimateSize: () => ESTIMATED_ROW_HEIGHT,
        overscan: OVERSCAN,
        scrollToFn: elementScroll,
        observeElementRect,
        observeElementOffset,
        onChange: (changed) => {
          setRows(changed.getVirtualItems());
          setTotalHeight(changed.getTotalSize());
        },
      });

      onCleanup(instance._didMount());
      instance._willUpdate();
      setVirtualizer(instance);
      setRows(instance.getVirtualItems());
      setTotalHeight(instance.getTotalSize());
    },
  );

  const measure = (element: HTMLDivElement): void => {
    queueMicrotask(() => virtualizer()?.measureElement(element));
  };

  return (
    <Show
      when={properties.analyses.length > 0}
      fallback={(
        <p class="rounded-lg border border-dashed border-slate-300 px-4 py-6 text-center text-sm text-slate-500 dark:border-slate-700 dark:text-slate-500">
          Nothing matches this filter.
        </p>
      )}
    >
      <div ref={setScroller} style={{ height: `${VISIBLE_HEIGHT}px` }} class="overflow-y-auto">
        <div class="relative w-full" style={{ height: `${totalHeight()}px` }}>
          <For each={rows()}>
            {item => (
              <div
                ref={measure}
                data-index={item.index}
                class="absolute top-0 left-0 w-full pb-2"
                style={{ transform: `translateY(${item.start}px)` }}
              >
                <Show when={properties.analyses[item.index]}>
                  {analysis => (
                    <SubjectRow
                      analysis={analysis()}
                      analyses={properties.all}
                      projectId={properties.projectId}
                      projectForge={properties.projectForge}
                    />
                  )}
                </Show>
              </div>
            )}
          </For>
        </div>
      </div>
    </Show>
  );
};
