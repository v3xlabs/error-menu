import { HoverCard } from "@kobalte/core/hover-card";
import { For, Show } from "solid-js";

import type { Analysis } from "../api/projects";
import type { ForgeKind, Person } from "../domain/analysis";
import { forgeItself, forgeOf, roleLabel, rolesOf, subjectLabel, subjectPath, subjectsOfPerson } from "../domain/analysis";
import { ForgeMark } from "./ForgeMark";
import { StatusDot } from "./StatusDot";

export type PersonAvatarProperties = {
  projectForge: ForgeKind | undefined;
  person: Person;
  analysis: Analysis;
  analyses: readonly Analysis[];
  projectId: string;
};

const OTHER_SUBJECT_LIMIT = 5;
const SHOWN_AVATARS = 4;

export const PersonAvatar = (properties: PersonAvatarProperties) => {
  const roles = (): readonly string[] =>
    rolesOf(properties.analysis, properties.person.identity).map(role => roleLabel(role));
  const elsewhere = (): readonly Analysis[] | undefined => {
    const others = subjectsOfPerson(properties.analyses, properties.person.identity).filter(
      analysis => analysis.snapshot_id !== properties.analysis.snapshot_id,
    );

    return others.length > 0 ? others : undefined;
  };

  return (
    <HoverCard>
      <HoverCard.Trigger
        as="span"
        tabindex="0"
        class="relative inline-flex rounded-full focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-500"
      >
        <Show
          when={forgeItself(properties.person)}
          fallback={(
            <>
              <img
                src={properties.person.avatar_path}
                alt={properties.person.label}
                width="24"
                height="24"
                loading="lazy"
                class="size-6 rounded-full bg-slate-200 ring-2 ring-white dark:bg-slate-800 dark:ring-slate-900"
              />
              <Show when={forgeOf(properties.person, properties.projectForge)}>
                {forge => (
                  <span class="absolute -right-0.5 -bottom-0.5 rounded-full bg-white p-px text-slate-700 dark:bg-slate-900 dark:text-slate-200">
                    <ForgeMark forge={forge()} size={10} />
                  </span>
                )}
              </Show>
            </>
          )}
        >
          {forge => (
            <span class="flex size-6 items-center justify-center rounded-full bg-slate-900 text-white ring-2 ring-white dark:bg-slate-100 dark:text-slate-900 dark:ring-slate-900">
              <ForgeMark forge={forge()} size={15} />
            </span>
          )}
        </Show>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content class="z-50 w-72 rounded-lg border border-slate-200 bg-white p-4 shadow-lg dark:border-slate-700 dark:bg-slate-900">
          <HoverCard.Arrow />
          <div class="flex items-center gap-3">
            <Show
              when={forgeItself(properties.person)}
              fallback={(
                <img
                  src={properties.person.avatar_path}
                  alt=""
                  width="40"
                  height="40"
                  class="size-10 rounded-full bg-slate-200 dark:bg-slate-800"
                />
              )}
            >
              {forge => (
                <span class="flex size-10 items-center justify-center rounded-full bg-slate-900 text-white dark:bg-slate-100 dark:text-slate-900">
                  <ForgeMark forge={forge()} size={15} />
                </span>
              )}
            </Show>
            <div class="min-w-0">
              <p class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100">{properties.person.label}</p>
              <Show when={properties.person.login}>
                {login => <p class="truncate text-xs text-slate-500 dark:text-slate-400">{login()}</p>}
              </Show>
              <Show when={properties.person.email}>
                {email => <p class="truncate font-mono text-xs text-slate-500 dark:text-slate-400">{email()}</p>}
              </Show>
            </div>
          </div>
          <p class="mt-3 text-xs text-slate-600 dark:text-slate-300">{roles().join(" - ")}</p>
          <Show when={elsewhere()}>
            {others => (
              <div class="mt-3 border-t border-slate-200 pt-3 dark:border-slate-700">
                <p class="text-xs font-medium tracking-wide text-slate-500 uppercase dark:text-slate-400">Also on</p>
                <ul class="mt-1.5 space-y-1">
                  <For each={others().slice(0, OTHER_SUBJECT_LIMIT)}>
                    {analysis => (
                      <li>
                        <a
                          href={subjectPath(properties.projectId, analysis)}
                          class="flex items-center gap-2 text-xs text-slate-600 hover:text-slate-900 dark:text-slate-300 dark:hover:text-slate-100"
                        >
                          <StatusDot tone={analysis.status} />
                          <span class="truncate">
                            {subjectLabel(analysis.subject)}
                            <Show when={analysis.forge.title}>
                              {title => <span class="text-slate-500 dark:text-slate-400">{` ${title()}`}</span>}
                            </Show>
                          </span>
                        </a>
                      </li>
                    )}
                  </For>
                </ul>
              </div>
            )}
          </Show>
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard>
  );
};

// One face per human, with the rest counted. A squashed branch can credit a dozen people
// and a row of twelve avatars stops being readable.
export const PersonRow = (properties: {
  projectForge: ForgeKind | undefined;
  people: readonly Person[];
  analysis: Analysis;
  analyses: readonly Analysis[];
  projectId: string;
}) => {
  const shown = (): readonly Person[] => properties.people.slice(0, SHOWN_AVATARS);
  const hidden = (): number | undefined => {
    const remaining = properties.people.length - shown().length;

    return remaining > 0 ? remaining : undefined;
  };
  const only = (): Person | undefined => (properties.people.length === 1 ? properties.people[0] : undefined);

  return (
    <span class="flex items-center gap-2">
      <span class="flex flex-row-reverse -space-x-1.5 space-x-reverse">
        <For each={shown().toReversed()}>
          {person => (
            <PersonAvatar
              person={person}
              analysis={properties.analysis}
              analyses={properties.analyses}
              projectId={properties.projectId}
              projectForge={properties.projectForge}
            />
          )}
        </For>
      </span>
      <Show when={only()}>
        {person => <span class="truncate text-xs text-slate-600 dark:text-slate-300">{person().label}</span>}
      </Show>
      <Show when={hidden()}>
        {count => (
          <span class="text-xs text-slate-500 dark:text-slate-400">{`+${count()}`}</span>
        )}
      </Show>
    </span>
  );
};
