import type { JSX } from "@solidjs/web";
import { FiAlertTriangle, FiFileText } from "solid-icons/fi";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { MOVEMENT_ICONS, MOVEMENT_TEXT, SEVERITY_TEXT } from "./Counts";

type Finding = Analysis["analyzers"][number]["findings"][number];

// A severity above Low outranks any movement: a package that arrived from nowhere is not
// an ordinary bump, whatever the version numbers did.
const ELEVATED: Record<string, string> = {
  critical: SEVERITY_TEXT.critical,
  high: SEVERITY_TEXT.high,
  medium: SEVERITY_TEXT.medium,
};

const style = (finding: Finding): string =>
  ELEVATED[finding.severity] ?? MOVEMENT_TEXT[finding.movement ?? "changed"];

const icon = (finding: Finding): JSX.Element => {
  if (ELEVATED[finding.severity] !== undefined) return <FiAlertTriangle size={12} />;

  if (finding.movement === undefined) return <FiFileText size={12} />;

  return MOVEMENT_ICONS[finding.movement]();
};

const location = (finding: Finding): string =>
  (finding.line_start === undefined ? finding.path : `${finding.path}:${finding.line_start}`);

export const FindingLine = (properties: { finding: Finding; showLocation?: boolean; }) => (
  <li class="flex items-baseline gap-1.5 text-xs">
    <span class={style(properties.finding)}>{icon(properties.finding)}</span>
    <span class={style(properties.finding)}>{properties.finding.detail}</span>
    <Show when={properties.showLocation === true && properties.finding.package === undefined}>
      <span class="shrink-0 font-mono text-slate-400 dark:text-slate-500">{location(properties.finding)}</span>
    </Show>
  </li>
);
