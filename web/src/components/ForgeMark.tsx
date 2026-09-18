import type { JSX } from "@solidjs/web";
import { FiGitBranch } from "solid-icons/fi";
import { SiGitea, SiGithub, SiGitlab } from "solid-icons/si";

import type { ForgeKind } from "../domain/analysis";

const MARKS: Record<ForgeKind, (size: number) => JSX.Element> = {
  github: size => <SiGithub size={size} />,
  gitlab: size => <SiGitlab size={size} />,
  gitea: size => <SiGitea size={size} />,
  forgejo: size => <SiGitea size={size} />,
};

// A project whose forge was never pinned still needs a glyph, and a branch mark says
// "a repository" without claiming which forge hosts it.
export const ForgeMark = (properties: { forge: ForgeKind | undefined; size: number; }) => {
  const mark = (): ((size: number) => JSX.Element) =>
    (properties.forge === undefined ? size => <FiGitBranch size={size} /> : MARKS[properties.forge]);

  return <>{mark()(properties.size)}</>;
};
