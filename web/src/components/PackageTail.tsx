import { FiBookOpen, FiGitBranch, FiPackage, FiShield } from "solid-icons/fi";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { CheckedLink } from "./CheckedLink";

type Package = NonNullable<Analysis["analyzers"][number]["findings"][number]["package"]>;
type Facts = NonNullable<Package["facts"]>;
type Advisory = Facts["advisories"][number];

// Matches the package audit, so a size shown in amber is a size the verdict counted.
const HEAVY_BYTES = 5_000_000;

const SEVERITY_RANK: Record<Advisory["severity"], number> = {
  critical: 0,
  high: 1,
  moderate: 2,
  unrated: 3,
  low: 4,
  informational: 5,
};

const MUTED = "shrink-0 font-mono text-slate-400 dark:text-slate-500";
const WARN = "shrink-0 font-mono text-amber-600 dark:text-amber-400";
const LINK = "inline-flex shrink-0 items-center text-slate-400 hover:text-slate-700 dark:text-slate-500 dark:hover:text-slate-200";
const ALARM_CHIP = "inline-flex shrink-0 items-center gap-0.5 whitespace-nowrap font-mono text-red-600 hover:underline dark:text-red-400";
const WARN_CHIP = "inline-flex shrink-0 items-center gap-0.5 whitespace-nowrap font-mono text-amber-600 hover:underline dark:text-amber-400";

const advisoryLine = (advisory: Advisory): string => {
  const names = [advisory.advisory_id, ...advisory.aliases].join(", ");
  const summary = advisory.summary === undefined ? "" : `: ${advisory.summary}`;
  const fixed = advisory.fixed_in === undefined ? "" : `, fixed in ${advisory.fixed_in}`;

  return `${advisory.severity}: ${names}${summary}${fixed}`;
};

// One chip however many advisories there are. It opens the worst one; the tooltip lists
// them all.
const Advisories = (properties: { name: string; advisories: readonly Advisory[]; }) => {
  const sorted = () => properties.advisories.toSorted((left, right) => SEVERITY_RANK[left.severity] - SEVERITY_RANK[right.severity]);

  return (
    <Show when={sorted()[0]}>
      {worst => (
        <CheckedLink
          link={worst().url}
          label={`${properties.name} advisories:\n${sorted()
            .map(advisoryLine)
            .join("\n")}`}
          class={SEVERITY_RANK[worst().severity] <= SEVERITY_RANK.high ? ALARM_CHIP : WARN_CHIP}
        >
          <FiShield size={11} />
          {`${sorted().length} ${sorted().length === 1 ? "advisory" : "advisories"}`}
        </CheckedLink>
      )}
    </Show>
  );
};

// Own size and install size read the same way, so a row never shows 5000 kB beside 5.0 MB.
const sizeText = (bytes: number): string =>
  (bytes >= 1_000_000 ? `${(bytes / 1_000_000).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1000))} kB`);

// The weight shown is the package's own, as the audit judges it. What its install tree adds
// sits on the dependency count, since the tree is what that number counts.
const FactChips = (properties: { name: string; facts: Facts; }) => (
  <>
    <Show when={properties.facts.size_bytes}>
      {bytes => (
        <span class={bytes() >= HEAVY_BYTES ? WARN : MUTED} title="Published size">
          {sizeText(bytes())}
        </span>
      )}
    </Show>
    <Show when={properties.facts.dependency_count}>
      {count => (
        <span
          class={MUTED}
          title={properties.facts.install_bytes === undefined ? "Packages its install pulls in" : `Installs ${sizeText(properties.facts.install_bytes)} with its dependencies`}
        >
          {`${count()} deps`}
        </span>
      )}
    </Show>
    <Advisories name={properties.name} advisories={properties.facts.advisories} />
    <Show when={properties.facts.withdrawn}>
      {notice => <span class={WARN} title={notice()}>withdrawn</span>}
    </Show>
  </>
);

// Fixed order, so a row does not reorder as facts arrive: weight, then breadth, then the
// things a reviewer stops for, then where to read more. A row whose sentence already links
// the name to its registry leaves the registry icon out.
export const PackageTail = (properties: { package: Package; withRegistry: boolean; }) => (
  <>
    <Show when={properties.package.facts}>
      {facts => <FactChips name={properties.package.name} facts={facts()} />}
    </Show>
    <Show when={properties.withRegistry ? properties.package.links.registry : undefined}>
      {link => (
        <CheckedLink link={link()} label={`${properties.package.name} on its registry`} class={LINK}>
          <FiPackage size={12} />
        </CheckedLink>
      )}
    </Show>
    <Show when={properties.package.links.docs}>
      {link => (
        <CheckedLink link={link()} label={`Documentation for ${properties.package.name}`} class={LINK}>
          <FiBookOpen size={12} />
        </CheckedLink>
      )}
    </Show>
    <Show when={properties.package.links.source}>
      {link => (
        <CheckedLink link={link()} label={`Source of ${properties.package.name}`} class={LINK}>
          <FiGitBranch size={12} />
        </CheckedLink>
      )}
    </Show>
  </>
);
