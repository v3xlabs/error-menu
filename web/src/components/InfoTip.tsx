import { Tooltip } from "@kobalte/core/tooltip";
import { FiInfo } from "solid-icons/fi";

export const InfoTip = (properties: { label: string; text: string; }) => (
  <Tooltip>
    <Tooltip.Trigger
      type="button"
      aria-label={properties.label}
      class="inline-flex text-slate-400 hover:text-slate-700 focus-visible:text-slate-700 dark:text-slate-500 dark:hover:text-slate-200 dark:focus-visible:text-slate-200"
    >
      <FiInfo size={13} />
    </Tooltip.Trigger>
    <Tooltip.Portal>
      <Tooltip.Content class="z-50 max-w-xs rounded-control bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900">
        <Tooltip.Arrow />
        {properties.text}
      </Tooltip.Content>
    </Tooltip.Portal>
  </Tooltip>
);
