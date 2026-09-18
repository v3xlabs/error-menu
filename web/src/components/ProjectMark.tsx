import { FiBox } from "solid-icons/fi";
import { Show } from "solid-js";

import type { components } from "../api/schema.gen";

type Project = components["schemas"]["ProjectOutput"];

export const ProjectMark = (properties: { project: Project; size: number; }) => (
  <Show
    when={properties.project.icon_light_path}
    fallback={(
      <span
        style={{ width: `${properties.size}px`, height: `${properties.size}px` }}
        class="flex shrink-0 items-center justify-center rounded-md bg-slate-100 text-slate-400 dark:bg-slate-800 dark:text-slate-500"
      >
        <FiBox size={Math.round(properties.size * 0.55)} />
      </span>
    )}
  >
    {path => (
      <img
        src={path()}
        alt=""
        width={properties.size}
        height={properties.size}
        style={{ width: `${properties.size}px`, height: `${properties.size}px` }}
        class="shrink-0 rounded-md object-contain"
      />
    )}
  </Show>
);
