import { createUniqueId, For } from "solid-js";

import type { Analyzer, AnalyzerId } from "../domain/analyzer";
import { ANALYZERS } from "../domain/analyzer";
import { InfoTip } from "./InfoTip";

// The label names its checkbox by id. Without that, the browser binds the label to the first
// labelable descendant, which is the tooltip button, and clicking the row does nothing.
const AnalyzerRow = (properties: {
  analyzer: Analyzer;
  isSelected: boolean;
  onToggle: (analyzerId: AnalyzerId) => void;
}) => {
  const checkbox_id = createUniqueId();

  return (
    <li>
      <label
        for={checkbox_id}
        class="flex cursor-pointer items-center gap-2.5 px-3 py-2 text-sm text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-800/60"
      >
        <span class="shrink-0 text-slate-500 dark:text-slate-400"><properties.analyzer.icon size={14} /></span>
        <span class="truncate font-medium">{properties.analyzer.label}</span>
        <InfoTip
          label={`What ${properties.analyzer.label} reports`}
          text={properties.analyzer.description}
        />
        <input
          id={checkbox_id}
          type="checkbox"
          class="ml-auto"
          checked={properties.isSelected}
          onInput={() => properties.onToggle(properties.analyzer.analyzerId)}
        />
      </label>
    </li>
  );
};

export const AnalyzerSelect = (properties: {
  selected: readonly AnalyzerId[];
  onToggle: (analyzerId: AnalyzerId) => void;
}) => (
  <ul class="divide-y divide-slate-200 rounded-md border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
    <For each={ANALYZERS}>
      {analyzer => (
        <AnalyzerRow
          analyzer={analyzer}
          isSelected={properties.selected.includes(analyzer.analyzerId)}
          onToggle={properties.onToggle}
        />
      )}
    </For>
  </ul>
);
