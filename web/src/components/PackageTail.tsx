import { FiBookOpen, FiGitBranch } from "solid-icons/fi";
import { For, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import { CheckedLink } from "./CheckedLink";

type Package = NonNullable<Analysis["analyzers"][number]["findings"][number]["package"]>;

// Fixed order, so a row does not reorder as facts arrive: weight, then breadth, then the
// two things a reviewer stops for.
const chips = (item: Package): readonly string[] => {
  const facts = item.facts;

  if (facts === undefined) return [];

  const size = facts.install_bytes ?? facts.size_bytes;
  const vulnerabilities = facts.vulnerabilities ?? 0;

  return [
    size === undefined ? undefined : `${Math.round(size / 1000).toLocaleString()} kB`,
    facts.dependency_count === undefined || facts.dependency_count === 0
      ? undefined
      : `${facts.dependency_count} deps`,
    vulnerabilities === 0 ? undefined : `${vulnerabilities} vulns`,
    facts.withdrawn === undefined ? undefined : "withdrawn",
  ].filter(chip => chip !== undefined);
};

const LINK = "inline-flex shrink-0 items-center text-slate-400 hover:text-slate-700 dark:text-slate-500 dark:hover:text-slate-200";

export const PackageTail = (properties: { package: Package; }) => (
  <>
    <For each={chips(properties.package)}>
      {chip => <span class="shrink-0 font-mono text-slate-400 dark:text-slate-500">{chip}</span>}
    </For>
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
