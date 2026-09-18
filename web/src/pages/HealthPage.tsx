import type { JSX } from "@solidjs/web";
import { createEffect, createSignal } from "solid-js";

import type { HealthState } from "../api/health";
import { fetchHealth } from "../api/health";

const renderHealthState = (state: HealthState): JSX.Element => {
  switch (state.phase) {
    case "loading": {
      return <p class="text-sm text-slate-500 dark:text-slate-500">Checking the backend...</p>;
    }
    case "error": {
      return <p class="text-sm text-red-600 dark:text-red-400">{state.message}</p>;
    }
    case "loaded": {
      return (
        <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
          <dt class="text-slate-500 dark:text-slate-500">Status</dt>
          <dd class="text-slate-900 dark:text-slate-100">{state.health.status}</dd>
          <dt class="text-slate-500 dark:text-slate-500">Version</dt>
          <dd class="text-slate-900 dark:text-slate-100">{state.health.version}</dd>
        </dl>
      );
    }
  }
};

export const HealthPage = () => {
  const [state, setState] = createSignal<HealthState>({ phase: "loading" });

  createEffect(
    () => undefined,
    () => {
      void fetchHealth().then(setState);
    },
  );

  return (
    <div class="space-y-4">
      <h1 class="text-lg font-semibold">Health</h1>
      <div class="rounded-lg border border-slate-200 p-4 dark:border-slate-800">{renderHealthState(state())}</div>
    </div>
  );
};
