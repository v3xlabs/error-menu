import { Tooltip } from "@kobalte/core/tooltip";
import { For } from "solid-js";

import type { DependencyFile, DependencyKind } from "../domain/dependency";
import { CountBar, CountRow, movementSlots } from "./Counts";
import { FileIcon } from "./FileIcon";

const KIND_LABELS: Record<DependencyKind, string> = {
  cargo: "cargo",
  nix: "nix",
  npm: "npm",
  pnpm: "pnpm",
  other: "dependency file",
};

const TIP = "z-50 rounded-md bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900";

const DependencyFileRow = (properties: { file: DependencyFile; }) => (
  <li class="flex items-center gap-3 px-3 py-2">
    <Tooltip>
      <Tooltip.Trigger as="span" class="flex shrink-0">
        <FileIcon kind={properties.file.kind} size={15} />
      </Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content class={TIP}>
          <Tooltip.Arrow />
          {KIND_LABELS[properties.file.kind]}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip>
    <span class="min-w-0 flex-1 truncate font-mono text-xs">
      <span class="text-slate-400 dark:text-slate-500">{properties.file.directory}</span>
      <span class="text-slate-900 dark:text-slate-100">{properties.file.name}</span>
    </span>
    <CountRow slots={movementSlots(properties.file.counts)} />
    <CountBar slots={movementSlots(properties.file.counts)} total={properties.file.total} />
  </li>
);

export const DependencyFileList = (properties: { files: readonly DependencyFile[]; }) => (
  <ul class="divide-y divide-slate-200 rounded-md border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
    <For each={properties.files}>{file => <DependencyFileRow file={file} />}</For>
  </ul>
);
