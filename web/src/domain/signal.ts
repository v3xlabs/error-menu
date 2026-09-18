import type { components } from "../api/schema.gen";

export type SignalKey = "off_task" | "diff_size" | "blast_radius" | "dependency_risk" | "tests_failing" | "links_added" | "repository_hygiene";

export type SignalValue
  = | { kind: "score"; value: number; }
    | { kind: "flag"; value: boolean; }
    | { kind: "count"; value: number; };

export type Signal = {
  key: SignalKey;
  value: SignalValue;
  reason: string;
};

const SIGNAL_LABELS: Record<SignalKey, string> = {
  off_task: "Off task",
  diff_size: "Diff size",
  blast_radius: "Blast radius",
  dependency_risk: "Dependency risk",
  tests_failing: "Tests failing",
  links_added: "Links added",
  repository_hygiene: "Repository hygiene",
};
const isSignalKey = (value: string): value is SignalKey => Object.hasOwn(SIGNAL_LABELS, value);

export const signalLabel = (key: SignalKey): string => SIGNAL_LABELS[key];

export const signalFromOutput = (signal: components["schemas"]["SignalOutput"]): Signal | undefined => {
  if (!isSignalKey(signal.key)) return undefined;

  switch (signal.value_kind) {
    case "score": {
      return signal.score === undefined
        ? undefined
        : { key: signal.key, value: { kind: "score", value: signal.score }, reason: signal.reason };
    }
    case "flag": {
      return signal.flag === undefined
        ? undefined
        : { key: signal.key, value: { kind: "flag", value: signal.flag }, reason: signal.reason };
    }
    case "count": {
      return signal.count === undefined
        ? undefined
        : { key: signal.key, value: { kind: "count", value: signal.count }, reason: signal.reason };
    }
  }
};

export type SignalConcern = "calm" | "elevated" | "alarming" | "neutral";

const SCORE_ELEVATED_THRESHOLD = 0.34;
const SCORE_ALARMING_THRESHOLD = 0.67;

export const signalConcern = (value: SignalValue): SignalConcern => {
  switch (value.kind) {
    case "score": {
      if (value.value >= SCORE_ALARMING_THRESHOLD) return "alarming";

      if (value.value >= SCORE_ELEVATED_THRESHOLD) return "elevated";

      return "calm";
    }
    case "flag": {
      return value.value ? "alarming" : "calm";
    }
    case "count": {
      return "neutral";
    }
  }
};

export const formatSignalValue = (value: SignalValue): string => {
  switch (value.kind) {
    case "score": {
      return `${Math.round(value.value * 100)}%`;
    }
    case "flag": {
      return value.value ? "yes" : "no";
    }
    case "count": {
      return String(value.value);
    }
  }
};
