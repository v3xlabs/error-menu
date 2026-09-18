import { Tooltip } from "@kobalte/core/tooltip";
import type { JSX } from "@solidjs/web";
import { FiAlertTriangle, FiCheck, FiClock, FiMinus } from "solid-icons/fi";

import type { StatusTone } from "../domain/analysis";

const PILL = "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium tabular-nums";

export const TONE_TINT: Record<StatusTone, string> = {
  alarming: "bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-300",
  attention: "bg-amber-100 text-amber-800 dark:bg-amber-900/50 dark:text-amber-300",
  clear: "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300",
  running: "bg-sky-100 text-sky-800 dark:bg-sky-900/50 dark:text-sky-300",
  unscanned: "bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-300",
};

const TONE_PILL: Record<StatusTone, string> = {
  alarming: `${PILL} ${TONE_TINT.alarming}`,
  attention: `${PILL} ${TONE_TINT.attention}`,
  clear: `${PILL} ${TONE_TINT.clear}`,
  running: `${PILL} ${TONE_TINT.running}`,
  unscanned: `${PILL} ${TONE_TINT.unscanned}`,
};

export const TONE_ICONS: Record<StatusTone, () => JSX.Element> = {
  alarming: () => <FiAlertTriangle size={12} />,
  attention: () => <FiAlertTriangle size={12} />,
  clear: () => <FiCheck size={12} />,
  running: () => <FiClock size={12} />,
  unscanned: () => <FiMinus size={12} />,
};

export const Gauge = (properties: { tone: StatusTone; value: number | string; label: string; icon?: JSX.Element; }) => (
  <Tooltip>
    <Tooltip.Trigger as="span" class={TONE_PILL[properties.tone]}>
      {properties.icon ?? TONE_ICONS[properties.tone]()}
      {properties.value}
    </Tooltip.Trigger>
    <Tooltip.Portal>
      <Tooltip.Content class="z-50 rounded-md bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900">
        <Tooltip.Arrow />
        {properties.label}
      </Tooltip.Content>
    </Tooltip.Portal>
  </Tooltip>
);
