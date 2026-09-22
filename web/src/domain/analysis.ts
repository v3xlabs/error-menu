import type { Analysis } from "../api/projects";

type Subject = Analysis["subject"];
type Finding = Analysis["analyzers"][number]["findings"][number];

// A run that is still in flight has no verdict yet, so it cannot be ranked with the rest.
export type StatusTone = "alarming" | "attention" | "clear" | "running" | "unscanned";

const TONE_RANK: Record<StatusTone, number> = {
  unscanned: 0,
  clear: 1,
  running: 2,
  attention: 3,
  alarming: 4,
};

export const analysisTone = (analysis: Analysis): StatusTone =>
  (analysis.analyzers.some(analyzer => analyzer.status === "running") ? "running" : analysis.status);

export const worstTone = (tones: readonly StatusTone[]): StatusTone | null =>
  tones.reduce<StatusTone | null>(
    (worst, current) => (worst === null || TONE_RANK[current] > TONE_RANK[worst] ? current : worst),
    null,
  );

export const subjectLabel = (subject: Subject): string => {
  switch (subject.kind) {
    case "change": {
      return `#${subject.key}`;
    }
    case "branch": {
      return subject.key;
    }
    case "commit": {
      return subject.key.slice(0, 12);
    }
  }
};

// A subject is read once per poll, so the same branch or pull request appears many times.
// Newest first is the order the API returns.
export const latestPerSubject = (analyses: readonly Analysis[]): readonly Analysis[] => {
  const seen = new Set<string>();

  return analyses.filter((analysis) => {
    const key = `${analysis.subject.kind}:${analysis.subject.key}`;

    if (seen.has(key)) return false;

    seen.add(key);

    return true;
  });
};

// A subject's history is one reading per commit it has pointed at. Scanning one commit
// more than once says nothing about the subject, so only the newest reading of a commit
// belongs in the list.
export const latestPerHead = (analyses: readonly Analysis[]): readonly Analysis[] => {
  const seen = new Set<string>();

  return analyses.filter((analysis) => {
    if (seen.has(analysis.head_sha)) return false;

    seen.add(analysis.head_sha);

    return true;
  });
};

export const findingsOf = (analysis: Analysis): readonly Finding[] =>
  analysis.analyzers.flatMap(analyzer => analyzer.findings);

export type Person = Analysis["people"][number];
export type PersonRole = Person["role"];

const ROLE_LABELS: Record<PersonRole, string> = {
  author: "Author",
  committer: "Committer",
  co_author: "Co-author",
  signed_off_by: "Signed off by",
  submitter: "Opened the change",
  reviewer: "Reviewer",
};

export const roleLabel = (role: PersonRole): string => ROLE_LABELS[role];

// The person who wrote the change leads the row. A committer is usually the same human,
// and when a rebase or a squash makes them different, the author is still the better name
// to show first.
const ROLE_ORDER: readonly PersonRole[] = ["author", "submitter", "committer", "co_author", "signed_off_by", "reviewer"];

export const orderedPeople = (analysis: Analysis): readonly Person[] =>
  analysis.people.toSorted((left, right) => ROLE_ORDER.indexOf(left.role) - ROLE_ORDER.indexOf(right.role));

// One human can hold several roles on the same change. The avatar row shows each human
// once, keeping the role that ranks first.
export const distinctPeople = (analysis: Analysis): readonly Person[] => {
  const seen = new Set<string>();

  return orderedPeople(analysis).filter((person) => {
    if (seen.has(person.identity)) return false;

    seen.add(person.identity);

    return true;
  });
};

export const rolesOf = (analysis: Analysis, identity: string): readonly PersonRole[] =>
  orderedPeople(analysis)
    .filter(person => person.identity === identity)
    .map(person => person.role);

export const subjectsOfPerson = (analyses: readonly Analysis[], identity: string): readonly Analysis[] =>
  latestPerSubject(analyses).filter(analysis => analysis.people.some(person => person.identity === identity));

export const subjectPath = (projectId: string, analysis: Analysis): string =>
  `/projects/${projectId}/${analysis.subject.kind}/${encodeURIComponent(analysis.subject.key)}`;

// A subject page shows its newest reading unless the URL names a head, so a link that
// means one commit has to carry that commit's sha.
export const scanPath = (projectId: string, analysis: Analysis): string =>
  `${subjectPath(projectId, analysis)}?head=${analysis.head_sha}`;

// The commit a merged change landed as. It links a default-branch commit back to the
// change that carried it.
export const mergedChangeFor = (analyses: readonly Analysis[], headSha: string): Analysis | undefined =>
  latestPerSubject(analyses).find(analysis => analysis.forge.merge_commit_sha === headSha);

// A closed or merged change is recorded for its history rather than scanned, so it never
// ran an analyzer. The timeline is about work error.menu actually did.
export const scanned = (analyses: readonly Analysis[]): readonly Analysis[] =>
  analyses.filter(analysis => analysis.analyzers.length > 0);

// An open change is the one a reviewer can still act on, so it leads the list, and a draft
// follows it because its author has not asked for review yet. Within a state the newest
// number is the most recent work.
const STATE_ORDER: Record<NonNullable<Analysis["forge"]["state"]>, number> = { open: 0, draft: 1, merged: 2, closed: 3 };

export const byReviewOrder = (analyses: readonly Analysis[]): readonly Analysis[] =>
  analyses.toSorted((left, right) => {
    const leftState = STATE_ORDER[left.forge.state ?? "open"];
    const rightState = STATE_ORDER[right.forge.state ?? "open"];

    if (leftState !== rightState) return leftState - rightState;

    return Number(right.subject.key) - Number(left.subject.key);
  });

const SEVERITY_TONES: Record<string, StatusTone> = {
  critical: "alarming",
  high: "alarming",
  medium: "attention",
  low: "clear",
  info: "clear",
};

export type ForgeKind = "github" | "gitlab" | "gitea" | "forgejo";

// Which forge a person belongs to, when that is knowable. A login comes from the forge API
// and so belongs to the project's forge. A commit identity carries only an address, and a
// forge noreply address is the one address that names its forge.
export const forgeOf = (person: Person, projectForge: ForgeKind | undefined): ForgeKind | undefined => {
  const email = person.email?.toLowerCase();

  if (email === "noreply@github.com" || email?.endsWith("@users.noreply.github.com") === true) return "github";

  if (email?.endsWith("@users.noreply.gitlab.com") === true) return "gitlab";

  return person.login === undefined ? undefined : projectForge;
};

export const forgeKind = (forge: string): ForgeKind | undefined =>
  (["github", "gitlab", "gitea", "forgejo"] as const).find(kind => forge === kind);

// The address a forge signs with when it makes the commit itself, on a squash or a merge
// from its web interface. That identity is the forge, not a person who works there, so it
// wears the forge's own mark rather than a picture with a mark beside it.
const FORGE_ADDRESSES: Record<string, ForgeKind> = {
  "noreply@github.com": "github",
  "noreply@gitlab.com": "gitlab",
};

export const forgeItself = (person: Person): ForgeKind | undefined =>
  (person.email === undefined ? undefined : FORGE_ADDRESSES[person.email.toLowerCase()]);

export type AnalyzerRun = Analysis["analyzers"][number];

// An analyzer's verdict uses the same ranking as the whole analysis, so a card and a row
// never disagree about what a colour means.
export const runTone = (run: AnalyzerRun): StatusTone => {
  if (run.status === "failed" || run.status === "timed_out") return "alarming";

  if (run.status === "running") return "running";

  if (run.status === "skipped") return "unscanned";

  return worstTone(run.findings.map(finding => SEVERITY_TONES[finding.severity] ?? "clear")) ?? "clear";
};

export type PackageFinding = AnalyzerRun["findings"][number];

export const forgeReference = (analysis: Analysis): string => {
  switch (analysis.subject.kind) {
    case "change": {
      return `#${analysis.subject.key}`;
    }
    case "branch": {
      return analysis.subject.key;
    }
    case "commit": {
      return analysis.subject.key.slice(0, 7);
    }
  }
};
