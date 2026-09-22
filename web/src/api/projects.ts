import { api } from "./client";
import type { components } from "./schema.gen";

export type Project = components["schemas"]["ProjectOutput"];
type CreateProject = components["schemas"]["CreateProject"];
export type Analysis = components["schemas"]["AnalysisOutput"];
export type Discovery = components["schemas"]["DiscoveryOutput"];
type AnalyzeProject = components["schemas"]["AnalyzeProject"];
type SetProjectAnalyzers = components["schemas"]["SetProjectAnalyzers"];

export type Result<Value> = { ok: true; value: Value; } | { ok: false; message: string; };

const failed = (status: number): Result<never> => ({ ok: false, message: `Request failed with status ${status}.` });

export const listProjects = async (): Promise<Result<readonly Project[]>> => {
  const response = await api("/projects", "get", {});

  if (response.status === 200) return { ok: true, value: response.data.projects };

  return failed(response.status);
};

export const readProject = async (projectId: string): Promise<Result<Project>> => {
  const response = await api("/projects/{project_id}", "get", { path: { project_id: projectId } });

  if (response.status === 200) return { ok: true, value: response.data };

  return failed(response.status);
};

export type Job = components["schemas"]["JobOutput"];

export const listJobs = async (projectId?: string): Promise<Result<readonly Job[]>> => {
  const response = await api("/jobs", "get", {
    query: projectId === undefined ? {} : { project_id: projectId },
  });

  if (response.status === 200) return { ok: true, value: response.data.jobs };

  switch (response.status) {
    case 400:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const listAnalyses = async (projectId: string): Promise<Result<readonly Analysis[]>> => {
  const response = await api("/projects/{project_id}/analyses", "get", { path: { project_id: projectId } });

  if (response.status === 200) return { ok: true, value: response.data.analyses };

  switch (response.status) {
    case 400: {
      return { ok: false, message: response.data.message };
    }
    case 404: {
      return { ok: false, message: response.data.message };
    }
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const createProject = async (project: CreateProject): Promise<Result<Project>> => {
  const response = await api("/projects", "post", {
    contentType: "application/json; charset=utf-8",
    data: project,
  });

  if (response.status === 201) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 403:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const moveProject = async (projectId: string, organizationId: string): Promise<Result<Project>> => {
  const response = await api("/projects/{project_id}/organization", "put", {
    path: { project_id: projectId },
    contentType: "application/json; charset=utf-8",
    data: { organization_id: organizationId },
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const runAnalysis = async (
  projectId: string,
  analysis: AnalyzeProject,
): Promise<Result<Analysis>> => {
  const response = await api("/projects/{project_id}/analyses", "post", {
    path: { project_id: projectId },
    contentType: "application/json; charset=utf-8",
    data: analysis,
  });

  if (response.status === 201) return { ok: true, value: response.data };

  switch (response.status) {
    case 400: {
      return { ok: false, message: response.data.message };
    }
    case 404: {
      return { ok: false, message: response.data.message };
    }
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const discoverProject = async (projectId: string): Promise<Result<Discovery>> => {
  const response = await api("/projects/{project_id}/discover", "post", { path: { project_id: projectId } });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400: {
      return { ok: false, message: response.data.message };
    }
    case 404: {
      return { ok: false, message: response.data.message };
    }
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export type IconCandidate = components["schemas"]["IconCandidateOutput"];

export const listIconCandidates = async (projectId: string): Promise<Result<readonly IconCandidate[]>> => {
  const response = await api("/projects/{project_id}/icon-candidates", "get", { path: { project_id: projectId } });

  if (response.status === 200) return { ok: true, value: response.data.candidates };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const setProjectIcon = async (
  projectId: string,
  icon: components["schemas"]["SetProjectIcon"],
): Promise<Result<Project>> => {
  const response = await api("/projects/{project_id}/icon", "put", {
    path: { project_id: projectId },
    contentType: "application/json; charset=utf-8",
    data: icon,
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const describeProject = async (projectId: string, description: string | null): Promise<Result<Project>> => {
  const response = await api("/projects/{project_id}/description", "put", {
    path: { project_id: projectId },
    contentType: "application/json; charset=utf-8",
    data: description === null ? {} : { description },
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const setProjectAnalyzers = async (
  projectId: string,
  analyzers: SetProjectAnalyzers,
): Promise<Result<Project>> => {
  const response = await api("/projects/{project_id}/analyzers", "put", {
    path: { project_id: projectId },
    contentType: "application/json; charset=utf-8",
    data: analyzers,
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const scanChange = async (projectId: string, number: number): Promise<Result<Analysis>> => {
  const response = await api("/projects/{project_id}/changes/{number}/scan", "post", {
    path: { project_id: projectId, number },
  });

  if (response.status === 201) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export type TreeEntry = components["schemas"]["TreeEntryOutput"];
type Tree = components["schemas"]["TreeOutput"];

export const readTree = async (projectId: string, path: string): Promise<Result<Tree>> => {
  const response = await api("/projects/{project_id}/tree", "get", {
    path: { project_id: projectId },
    query: { path },
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export type Commit = components["schemas"]["CommitOutput"];

export const listCommits = async (projectId: string, limit: number): Promise<Result<readonly Commit[]>> => {
  const response = await api("/projects/{project_id}/commits", "get", {
    path: { project_id: projectId },
    query: { limit },
  });

  if (response.status === 200) return { ok: true, value: response.data.commits };

  switch (response.status) {
    case 400:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export type ProjectMember = components["schemas"]["ProjectMemberOutput"];
export type ProjectRole = components["schemas"]["ProjectRoleOutput"];

export const listProjectMembers = async (projectId: string): Promise<Result<readonly ProjectMember[]>> => {
  const response = await api("/projects/{project_id}/members", "get", { path: { project_id: projectId } });

  if (response.status === 200) return { ok: true, value: response.data.members };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const setProjectMember = async (
  projectId: string,
  userId: string,
  role: ProjectRole,
): Promise<Result<readonly ProjectMember[]>> => {
  const response = await api("/projects/{project_id}/members/{user_id}", "put", {
    path: { project_id: projectId, user_id: userId },
    contentType: "application/json; charset=utf-8",
    data: { role },
  });

  if (response.status === 200) return { ok: true, value: response.data.members };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const removeProjectMember = async (
  projectId: string,
  userId: string,
): Promise<Result<readonly ProjectMember[]>> => {
  const response = await api("/projects/{project_id}/members/{user_id}", "delete", {
    path: { project_id: projectId, user_id: userId },
  });

  if (response.status === 200) return { ok: true, value: response.data.members };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};
