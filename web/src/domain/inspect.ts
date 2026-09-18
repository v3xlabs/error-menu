export type InspectTarget
  = | { kind: "change"; number: number; }
    | { kind: "commit"; head: string; }
    | { kind: "range"; base: string; head: string; };

// Abbreviated shas are rejected by the API, because a fingerprint that compares against one
// cannot be reproduced later. So the dialog rejects them here, with a message.
const FULL_SHA = /^[0-9a-f]{40}$/;
const CHANGE_SEGMENTS = new Set(["pull", "pulls", "merge_requests"]);
const COMMIT_SEGMENTS = new Set(["commit", "commits"]);
const NUMBER = /^\d+$/;

const revisionPair = (text: string): InspectTarget | null => {
  const [base, head] = text.split(/\.{2,3}/, 2);

  if (base !== undefined && head !== undefined && FULL_SHA.test(base) && FULL_SHA.test(head)) {
    return { kind: "range", base, head };
  }

  return FULL_SHA.test(text) ? { kind: "commit", head: text } : null;
};

export const parseInspectInput = (input: string): InspectTarget | null => {
  const text = input.trim();
  const url = URL.parse(text);
  const segments = (url === null ? text : url.pathname).split("/").filter(segment => segment.length > 0);

  for (const [index, segment] of segments.entries()) {
    const value = segments[index + 1];

    if (value === undefined) continue;

    if (CHANGE_SEGMENTS.has(segment) && NUMBER.test(value)) return { kind: "change", number: Number(value) };

    if (COMMIT_SEGMENTS.has(segment) && FULL_SHA.test(value)) return { kind: "commit", head: value };

    if (segment === "compare") return revisionPair(value);
  }

  const only = segments.length === 1 ? segments.at(0) : undefined;

  return only === undefined ? null : revisionPair(only);
};
