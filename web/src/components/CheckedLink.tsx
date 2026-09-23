import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { components } from "../api/schema.gen";

type Link = components["schemas"]["LinkOutput"];

const SUSPICIOUS = "inline-flex shrink-0 items-center cursor-not-allowed border-b-2 border-dotted border-red-500 text-red-600 dark:text-red-400";

// A suspicious link keeps its text on screen and loses its href: the reviewer has to see
// what was attempted, and nothing on this page may follow it.
export const CheckedLink = (properties: {
  link: Link;
  label: string;
  class: string;
  children: JSX.Element;
}) => (
  <Show
    when={properties.link.status === "safe"}
    fallback={(
      <span
        role="link"
        aria-disabled="true"
        class={SUSPICIOUS}
        title={`Blocked suspicious link for ${properties.label}: ${properties.link.url}`}
      >
        {properties.children}
      </span>
    )}
  >
    <a
      href={properties.link.url}
      target="_blank"
      rel="noreferrer"
      class={properties.class}
      title={properties.label}
    >
      {properties.children}
    </a>
  </Show>
);
