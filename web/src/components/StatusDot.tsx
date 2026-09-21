import type { StatusTone } from "../domain/analysis";

const TONE_STYLES: Record<StatusTone, string> = {
  alarming: "inline-block size-2.5 shrink-0 rounded-full bg-red-500",
  attention: "inline-block size-2.5 shrink-0 rounded-full bg-amber-500",
  clear: "inline-block size-2.5 shrink-0 rounded-full bg-emerald-500",
  running: "inline-block size-2.5 shrink-0 rounded-full bg-sky-500",
  unscanned: "inline-block size-2.5 shrink-0 rounded-full border border-slate-400 bg-transparent",
};

const TONE_LABELS: Record<StatusTone, string> = {
  alarming: "Alarming",
  attention: "Needs attention",
  clear: "Nothing to flag",
  running: "Running",
  unscanned: "Not scanned",
};

export const toneLabel = (tone: StatusTone): string => TONE_LABELS[tone];

export const StatusDot = (properties: { tone: StatusTone; }) => (
  <span role="img" aria-label={TONE_LABELS[properties.tone]} class={TONE_STYLES[properties.tone]} />
);
