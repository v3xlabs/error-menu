import type { PackageFinding } from "./analysis";

export type DependencyMovement = NonNullable<PackageFinding["movement"]>;

export type DependencyKind = "cargo" | "nix" | "npm" | "pnpm" | "other";

const KIND_BY_NAME: Record<string, DependencyKind> = {
  "Cargo.lock": "cargo",
  "Cargo.toml": "cargo",
  "flake.lock": "nix",
  "package-lock.json": "npm",
  "package.json": "npm",
  "pnpm-lock.yaml": "pnpm",
};

export type DependencyCounts = Record<DependencyMovement, number>;

// One dependency file of one snapshot. A repository holds as many as it has workspaces, and
// two of them can be the same kind, so the path is the identity.
export type DependencyFile = {
  path: string;
  directory: string;
  name: string;
  kind: DependencyKind;
  counts: DependencyCounts;
  total: number;
};

const emptyCounts = (): DependencyCounts => ({ added: 0, removed: 0, upgraded: 0, downgraded: 0, changed: 0 });

export const nameOf = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

export const kindOf = (path: string): DependencyKind => KIND_BY_NAME[nameOf(path)] ?? "other";

const fileOf = (path: string): DependencyFile => ({
  path,
  directory: path.slice(0, path.lastIndexOf("/") + 1),
  name: nameOf(path),
  kind: kindOf(path),
  counts: emptyCounts(),
  total: 0,
});

// A finding without a direction is a change the analyzer could not order, which is what a
// flake.lock pinning one commit over another reports.
export const dependencyFiles = (findings: readonly PackageFinding[]): readonly DependencyFile[] => {
  const byPath = new Map<string, DependencyFile>();
  const files: DependencyFile[] = [];

  for (const finding of findings) {
    if (finding.package === undefined) continue;

    let file = byPath.get(finding.path);

    if (file === undefined) {
      file = fileOf(finding.path);
      byPath.set(finding.path, file);
      files.push(file);
    }

    file.counts[finding.movement ?? "changed"] += 1;
    file.total += 1;
  }

  return files.toSorted((left, right) => right.total - left.total || left.path.localeCompare(right.path));
};

export const dependencyTotals = (files: readonly DependencyFile[]): DependencyCounts =>
  files.reduce<DependencyCounts>((totals, file) => {
    totals.added += file.counts.added;
    totals.removed += file.counts.removed;
    totals.upgraded += file.counts.upgraded;
    totals.downgraded += file.counts.downgraded;
    totals.changed += file.counts.changed;

    return totals;
  }, emptyCounts());
