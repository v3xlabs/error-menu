import { api } from "./client";
import type { components } from "./schema.gen";

export type Health = components["schemas"]["Health"];

export type HealthState = { phase: "loading"; } | { phase: "loaded"; health: Health; } | { phase: "error"; message: string; };

export const fetchHealth = async (): Promise<HealthState> => {
  const response = await api("/health", "get", {});

  if (response.status === 200) return { phase: "loaded", health: response.data };

  return { phase: "error", message: `Request failed with status ${response.status}.` };
};
