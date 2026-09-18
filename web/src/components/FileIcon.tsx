import type { DependencyKind } from "../domain/dependency";

// The symbols live in index.html, because a `use` reference only resolves inside its own
// document. The artwork is the vscode-icons set, so a dependency file wears the same mark
// here as it does in the editor.
const KIND_SYMBOLS: Record<DependencyKind, string> = {
  cargo: "#vsi-cargo",
  nix: "#vsi-nix",
  npm: "#vsi-npm",
  pnpm: "#vsi-pnpm",
  other: "#vsi-file",
};

export const FileIcon = (properties: { kind: DependencyKind; size?: number; }) => (
  <svg
    width={properties.size ?? 16}
    height={properties.size ?? 16}
    viewBox="0 0 32 32"
    aria-hidden="true"
  >
    <use href={KIND_SYMBOLS[properties.kind]} />
  </svg>
);
