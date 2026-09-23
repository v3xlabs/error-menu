import type { JSX } from "@solidjs/web";
import { FiAlertTriangle, FiFileText } from "solid-icons/fi";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { CheckedLink } from "./CheckedLink";
import { MOVEMENT_ICONS, MOVEMENT_TEXT, SEVERITY_TEXT } from "./Counts";
import { PackageTail } from "./PackageTail";

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

// The sentence already names the package, so the registry link is that name rather than a
// second copy of it beside the row.
const detail = (finding: Finding): JSX.Element => {
  const item = finding.package;
  const registry = item?.links.registry;

  if (item === undefined || registry === undefined) return finding.detail;

  const at = finding.detail.indexOf(item.name);

  if (at === -1) return finding.detail;

  return (
    <>
      {finding.detail.slice(0, at)}
      <CheckedLink
        link={registry}
        label={`${item.name} on its registry`}
        class="underline decoration-slate-300 underline-offset-2 dark:decoration-slate-600"
      >
        {item.name}
      </CheckedLink>
      {finding.detail.slice(at + item.name.length)}
    </>
  );
};

export const FindingLine = (properties: { finding: Finding; showLocation?: boolean; }) => (
  <li class="flex items-baseline gap-1.5 text-xs">
    <span class={style(properties.finding)}>{icon(properties.finding)}</span>
    <span class={style(properties.finding)}>{detail(properties.finding)}</span>
    <Show when={properties.finding.package}>
      {item => <PackageTail package={item()} />}
    </Show>
    <Show when={properties.showLocation === true && properties.finding.package === undefined}>
      <span class="shrink-0 font-mono text-slate-400 dark:text-slate-500">{location(properties.finding)}</span>
    </Show>
  </li>
);

// Name and version read as columns down a file; the sentence stays for screen readers and on
// hover, since an upgrade names its old version only there.
export const PackageLine = (properties: { finding: Finding; name: string; version: string; }) => (
  <li class={["grid grid-cols-[12px_minmax(0,1fr)_auto] items-baseline gap-2.5 font-mono text-xs", style(properties.finding)]} title={properties.finding.detail}>
    <span aria-hidden="true">{icon(properties.finding)}</span>
    <span aria-hidden="true" class="truncate">{properties.name}</span>
    <span aria-hidden="true" class="tabular-nums opacity-80">{properties.version}</span>
    <span class="sr-only">{properties.finding.detail}</span>
  </li>
);
