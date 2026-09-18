import { Tooltip } from "@kobalte/core/tooltip";
import { FiLock, FiShield } from "solid-icons/fi";
import { Show } from "solid-js";

import type { Analysis } from "../api/projects";

type Signature = Analysis["signature"];

// Three states, and the middle one matters: a commit can carry a signature that the forge
// declines to vouch for. error.menu checks no cryptography itself, so it never claims more
// than the forge told it.
const verdict = (signature: Signature): { style: string; label: string; } | undefined => {
  if (!signature.present) return undefined;

  if (signature.verified === true) {
    return {
      style: "inline-flex items-center gap-0.5 text-xs text-emerald-600 dark:text-emerald-400",
      label: signature.signer === undefined
        ? "Signed, and the forge verified the key"
        : `Signed by ${signature.signer}, and the forge verified the key`,
    };
  }

  return {
    style: "inline-flex items-center gap-0.5 text-xs text-slate-400 dark:text-slate-500",
    label: signature.reason === undefined
      ? "Signed, but no forge has verified the key"
      : `Signed, but the forge did not verify it: ${signature.reason}`,
  };
};

export const SignatureBadge = (properties: { signature: Signature; }) => (
  <Show when={verdict(properties.signature)}>
    {state => (
      <Tooltip>
        <Tooltip.Trigger as="span" class={state().style}>
          <Show when={properties.signature.verified === true} fallback={<FiLock size={12} />}>
            <FiShield size={12} />
          </Show>
        </Tooltip.Trigger>
        <Tooltip.Portal>
          <Tooltip.Content class="z-50 max-w-xs rounded-md bg-slate-900 px-2.5 py-1.5 text-xs text-white dark:bg-slate-100 dark:text-slate-900">
            <Tooltip.Arrow />
            {state().label}
          </Tooltip.Content>
        </Tooltip.Portal>
      </Tooltip>
    )}
  </Show>
);
