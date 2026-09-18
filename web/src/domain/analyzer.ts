import type { IconTypes } from "solid-icons";
import { VsChecklist, VsFileCode, VsFolder, VsKey, VsLaw, VsLink, VsPackage, VsPlayCircle } from "solid-icons/vs";

export type AnalyzerId
  = | "lockfile_delta"
    | "manifest_delta"
    | "secret_scan"
    | "link_inventory"
    | "workflow_security"
    | "repository_controls"
    | "ci_check_runs"
    | "repository_hygiene";

// Two identifiers for one analyzer: `analyzerId` is the enum the API takes when a project
// picks its analyzers, and `runId` is what a run and a finding report. Everything a reader
// sees about an analyzer comes from this one table.
export type Analyzer = {
  analyzerId: AnalyzerId;
  runId: string;
  label: string;
  description: string;
  icon: IconTypes;
};

export const ANALYZERS: readonly Analyzer[] = [
  { analyzerId: "lockfile_delta", runId: "lockfile-delta", label: "Lockfile delta", description: "Detects unregistered dependencies, source changes, and integrity changes in supported lockfiles.", icon: VsPackage },
  { analyzerId: "manifest_delta", runId: "manifest-delta", label: "Manifest delta", description: "Reports dependency additions, removals, and constraint changes in Cargo and npm manifests.", icon: VsFileCode },
  { analyzerId: "secret_scan", runId: "secret-scan", label: "Secret scan", description: "Detects credential-shaped values in lines added by a change.", icon: VsKey },
  { analyzerId: "link_inventory", runId: "link-inventory", label: "Link inventory", description: "Reports links added anywhere in the repository without rewriting them.", icon: VsLink },
  { analyzerId: "workflow_security", runId: "workflow-security", label: "Workflow security", description: "Reports unpinned actions, unsafe checkout patterns, write permissions, and downloaded shell pipelines.", icon: VsPlayCircle },
  { analyzerId: "repository_controls", runId: "repository-controls", label: "Repository controls", description: "Reports changes to repository ownership, attribute, submodule, and package-manager controls.", icon: VsLaw },
  { analyzerId: "ci_check_runs", runId: "ci-check-runs", label: "CI checks", description: "Reads recorded CI check outcomes from the configured forge.", icon: VsChecklist },
  { analyzerId: "repository_hygiene", runId: "repository-hygiene", label: "Repository hygiene", description: "Reports changed generated output and large files.", icon: VsFolder },
];

export const analyzerByRunId = (runId: string): Analyzer | undefined =>
  ANALYZERS.find(analyzer => analyzer.runId === runId);
