import type { Signal, SignalConcern } from "../domain/signal";
import { formatSignalValue, signalConcern, signalLabel } from "../domain/signal";

const CONTAINER_STYLES: Record<SignalConcern, string> = {
  calm: "rounded-lg border px-3 py-2 border-emerald-200 bg-emerald-50 dark:border-emerald-900 dark:bg-emerald-950/40",
  elevated: "rounded-lg border px-3 py-2 border-amber-200 bg-amber-50 dark:border-amber-900 dark:bg-amber-950/40",
  alarming: "rounded-lg border px-3 py-2 border-red-200 bg-red-50 dark:border-red-900 dark:bg-red-950/40",
  neutral: "rounded-lg border px-3 py-2 border-slate-200 bg-slate-50 dark:border-slate-800 dark:bg-slate-900/40",
};

const VALUE_STYLES: Record<SignalConcern, string> = {
  calm: "text-sm font-semibold text-emerald-700 dark:text-emerald-300",
  elevated: "text-sm font-semibold text-amber-700 dark:text-amber-300",
  alarming: "text-sm font-semibold text-red-700 dark:text-red-300",
  neutral: "text-sm font-semibold text-slate-700 dark:text-slate-300",
};

export const SignalBadge = (properties: { signal: Signal; }) => (
  <div class={CONTAINER_STYLES[signalConcern(properties.signal.value)]}>
    <div class="flex items-baseline justify-between gap-3">
      <span class="text-xs font-medium tracking-wide text-slate-500 uppercase dark:text-slate-400">
        {signalLabel(properties.signal.key)}
      </span>
      <span class={VALUE_STYLES[signalConcern(properties.signal.value)]}>
        {formatSignalValue(properties.signal.value)}
      </span>
    </div>
    <p class="mt-1 text-sm text-slate-600 dark:text-slate-400">{properties.signal.reason}</p>
  </div>
);
