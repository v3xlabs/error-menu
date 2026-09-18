import type { PackageFinding } from "./analysis";
import type { DependencyKind } from "./dependency";
import { kindOf, nameOf } from "./dependency";

export const SEVERITY_ORDER = ["critical", "high", "medium", "low", "info"] as const;

export type Severity = typeof SEVERITY_ORDER[number];

export type SeverityCounts = Record<Severity, number>;

// A finding the backend labelled with something this build does not know about is counted as
// info rather than dropped, because a finding nobody counted is a finding nobody reads.
const severityOf = (value: string): Severity =>
  SEVERITY_ORDER.find(severity => severity === value) ?? "info";

const emptyCounts = (): SeverityCounts => ({ critical: 0, high: 0, medium: 0, low: 0, info: 0 });

export const severityCounts = (findings: readonly PackageFinding[]): SeverityCounts => {
  const counts = emptyCounts();

  for (const finding of findings) {
    counts[severityOf(finding.severity)] += 1;
  }

  return counts;
};

export type SeverityFile = {
  path: string;
  directory: string;
  name: string;
  kind: DependencyKind;
  counts: SeverityCounts;
  total: number;
  findings: readonly PackageFinding[];
};

export const severityFiles = (findings: readonly PackageFinding[]): readonly SeverityFile[] => {
  const byPath = new Map<string, SeverityFile & { findings: PackageFinding[]; }>();
  const files: (SeverityFile & { findings: PackageFinding[]; })[] = [];

  for (const finding of findings) {
    let file = byPath.get(finding.path);

    if (file === undefined) {
      const separator = finding.path.lastIndexOf("/");

      file = {
        path: finding.path,
        directory: finding.path.slice(0, separator + 1),
        name: nameOf(finding.path),
        kind: kindOf(finding.path),
        counts: emptyCounts(),
        total: 0,
        findings: [],
      };
      byPath.set(finding.path, file);
      files.push(file);
    }

    file.counts[severityOf(finding.severity)] += 1;
    file.total += 1;
    file.findings.push(finding);
  }

  return files.toSorted((left, right) => right.total - left.total || left.path.localeCompare(right.path));
};
