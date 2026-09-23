import { Tooltip } from "@kobalte/core/tooltip";
import type { JSX } from "@solidjs/web";
import { FiAlertTriangle, FiArrowDown, FiArrowRight, FiArrowUp, FiInfo, FiMinus, FiPlus } from "solid-icons/fi";
import { For, Show } from "solid-js";

import type { DependencyCounts, DependencyMovement } from "../domain/dependency";
import type { Severity, SeverityCounts } from "../domain/severity";
import { SEVERITY_ORDER } from "../domain/severity";

export const MOVEMENT_TEXT: Record<DependencyMovement, string> = {
  upgraded: "text-sky-700 dark:text-sky-400",
  downgraded: "text-amber-700 dark:text-amber-400",
  added: "text-emerald-700 dark:text-emerald-400",
  removed: "text-red-700 dark:text-red-400",
  changed: "text-slate-600 dark:text-slate-300",
};

const MOVEMENT_FILL: Record<DependencyMovement, string> = {
  upgraded: "bg-sky-500",
  downgraded: "bg-amber-500",
  added: "bg-emerald-500",
  removed: "bg-red-500",
  changed: "bg-slate-300 dark:bg-slate-600",
};

export const MOVEMENT_ICONS: Record<DependencyMovement, () => JSX.Element> = {
  upgraded: () => <FiArrowUp size={12} />,
  downgraded: () => <FiArrowDown size={12} />,
  added: () => <FiPlus size={12} />,
  removed: () => <FiMinus size={12} />,
  changed: () => <FiArrowRight size={12} />,
};

const MOVEMENT_LABELS: Record<DependencyMovement, string> = {
  upgraded: "upgraded",
  downgraded: "downgraded",
  added: "added",
  removed: "removed",
  changed: "changed without an order",
};

// Read in the order a reviewer asks about: which versions moved, then what arrived or left,
// then what changed without an order.
const MOVEMENTS: readonly DependencyMovement[] = ["upgraded", "downgraded", "added", "removed", "changed"];

export const SEVERITY_TEXT: Record<Severity, string> = {
  critical: "text-red-700 dark:text-red-400",
  high: "text-red-700 dark:text-red-400",
  medium: "text-amber-700 dark:text-amber-400",
  low: "text-sky-700 dark:text-sky-400",
  info: "text-slate-500 dark:text-slate-400",
};

const SEVERITY_FILL: Record<Severity, string> = {
  critical: "bg-red-600",
  high: "bg-red-500",
  medium: "bg-amber-500",
  low: "bg-sky-500",
  info: "bg-slate-300 dark:bg-slate-600",
};

const SEVERITY_ICONS: Record<Severity, () => JSX.Element> = {
  critical: () => <FiAlertTriangle size={12} />,
  high: () => <FiAlertTriangle size={12} />,
  medium: () => <FiAlertTriangle size={12} />,
  low: () => <FiInfo size={12} />,
  info: () => <FiInfo size={12} />,
};

const COUNT = "flex items-center justify-end gap-0.5 text-xs tabular-nums";

export type CountSlot = {
  key: string;
  value: number;
  label: string;
  countClass: string;
  fill: string;
  icon: () => JSX.Element;
};

export const movementSlots = (counts: DependencyCounts): readonly CountSlot[] =>
  MOVEMENTS.map(movement => ({
    key: movement,
    value: counts[movement],
    label: MOVEMENT_LABELS[movement],
    countClass: `${COUNT} ${MOVEMENT_TEXT[movement]}`,
    fill: MOVEMENT_FILL[movement],
    icon: MOVEMENT_ICONS[movement],
  }));

export const severitySlots = (counts: SeverityCounts): readonly CountSlot[] =>
  SEVERITY_ORDER.map(severity => ({
    key: severity,
    value: counts[severity],
    label: severity,
    countClass: `${COUNT} ${SEVERITY_TEXT[severity]}`,
    fill: SEVERITY_FILL[severity],
    icon: SEVERITY_ICONS[severity],
  }));

const TIP = "z-50 rounded-control bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900";

// One slot per kind, always in the same place and always the same width, so a column of
// rows can be read down rather than one row at a time.
export const CountRow = (properties: { slots: readonly CountSlot[]; }) => (
  <span class="flex shrink-0 items-center">
    <For each={properties.slots}>
      {slot => (
        <span class="w-12">
          <Show when={slot.value > 0}>
            <Tooltip>
              <Tooltip.Trigger as="span" class={slot.countClass}>
                {slot.icon()}
                {slot.value}
              </Tooltip.Trigger>
              <Tooltip.Portal>
                <Tooltip.Content class={TIP}>
                  <Tooltip.Arrow />
                  {`${slot.value} ${slot.label}`}
                </Tooltip.Content>
              </Tooltip.Portal>
            </Tooltip>
          </Show>
        </span>
      )}
    </For>
  </span>
);

export const CountBar = (properties: { slots: readonly CountSlot[]; total: number; }) => (
  <span class="hidden h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-slate-100 sm:flex dark:bg-slate-800">
    <For each={properties.slots}>
      {slot => (
        <Show when={slot.value > 0}>
          <span class={slot.fill} style={{ width: `${(slot.value / properties.total) * 100}%` }} />
        </Show>
      )}
    </For>
  </span>
);
