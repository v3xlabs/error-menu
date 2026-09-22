import { Tooltip } from "@kobalte/core/tooltip";
import { VsError, VsHistory, VsSync, VsWatch } from "solid-icons/vs";
import { Match, Switch } from "solid-js";

import type { QueueState } from "../domain/job";
import { TONE_TINT } from "./Gauge";

const PILL = "inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium";
const TIP = "z-50 max-w-sm rounded-control bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900";

const RUNNING = `${PILL} ${TONE_TINT.running}`;
const QUEUED = `${PILL} ${TONE_TINT.unscanned}`;
const FAILED = `${PILL} ${TONE_TINT.alarming}`;
const DONE = "inline-flex items-center gap-1.5 text-xs text-slate-500 dark:text-slate-400";

export const QueueStateBadge = (properties: { state: QueueState; }) => (
  <Switch>
    <Match when={properties.state.phase === "running"}>
      <span class={RUNNING}>
        <VsSync size={12} />
        Scanning
      </span>
    </Match>
    <Match when={properties.state.phase === "queued" ? properties.state : undefined}>
      {queued => (
        <Tooltip>
          <Tooltip.Trigger as="span" class={QUEUED}>
            <VsHistory size={12} />
            Queued
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content class={TIP}>
              <Tooltip.Arrow />
              {queued().retryOf ?? "Waiting for the queue."}
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip>
      )}
    </Match>
    <Match when={properties.state.phase === "waiting" ? properties.state : undefined}>
      {waiting => (
        <Tooltip>
          <Tooltip.Trigger as="span" class={QUEUED}>
            <VsWatch size={12} />
            Waiting
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content class={TIP}>
              <Tooltip.Arrow />
              {`${waiting().reason ?? "Waiting for its turn."} Next try at ${new Date(waiting().untilIso).toLocaleTimeString()}.`}
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip>
      )}
    </Match>
    <Match when={properties.state.phase === "failed" ? properties.state : undefined}>
      {failed => (
        <Tooltip>
          <Tooltip.Trigger as="span" class={FAILED}>
            <VsError size={12} />
            {`Scan failed after ${failed().attempts} ${failed().attempts === 1 ? "try" : "tries"}`}
          </Tooltip.Trigger>
          <Tooltip.Portal>
            <Tooltip.Content class={TIP}>
              <Tooltip.Arrow />
              {failed().error ?? "No reason was recorded."}
            </Tooltip.Content>
          </Tooltip.Portal>
        </Tooltip>
      )}
    </Match>
    <Match when={properties.state.phase === "done" ? properties.state : undefined}>
      {done => (
        <span class={DONE}>
          <VsWatch size={12} />
          {`Scanned ${new Date(done().at).toLocaleString()}`}
        </span>
      )}
    </Match>
  </Switch>
);
