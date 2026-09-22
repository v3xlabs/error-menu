import { api } from "./client";
import type { Result } from "./projects";
import type { components } from "./schema.gen";

export type Organization = components["schemas"]["OrganizationOutput"];
export type OrganizationMember = components["schemas"]["OrganizationMemberOutput"];
export type OrganizationRole = components["schemas"]["OrganizationRoleOutput"];
type CreateOrganization = components["schemas"]["CreateOrganization"];

const failed = (status: number): Result<never> => ({ ok: false, message: `Request failed with status ${status}.` });

export const listOrganizations = async (): Promise<Result<readonly Organization[]>> => {
  const response = await api("/orgs", "get", {});

  if (response.status === 200) return { ok: true, value: response.data.organizations };

  switch (response.status) {
    case 401:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const readOrganization = async (organizationId: string): Promise<Result<Organization>> => {
  const response = await api("/orgs/{organization_id}", "get", { path: { organization_id: organizationId } });

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

export const createOrganization = async (organization: CreateOrganization): Promise<Result<Organization>> => {
  const response = await api("/orgs", "post", {
    contentType: "application/json; charset=utf-8",
    data: organization,
  });

  if (response.status === 201) return { ok: true, value: response.data };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const describeOrganization = async (
  organizationId: string,
  organization: CreateOrganization,
): Promise<Result<Organization>> => {
  const response = await api("/orgs/{organization_id}", "put", {
    path: { organization_id: organizationId },
    contentType: "application/json; charset=utf-8",
    data: organization,
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

export const listOrganizationMembers = async (
  organizationId: string,
): Promise<Result<readonly OrganizationMember[]>> => {
  const response = await api("/orgs/{organization_id}/members", "get", {
    path: { organization_id: organizationId },
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

export const setOrganizationMember = async (
  organizationId: string,
  userId: string,
  role: OrganizationRole,
): Promise<Result<readonly OrganizationMember[]>> => {
  const response = await api("/orgs/{organization_id}/members/{user_id}", "put", {
    path: { organization_id: organizationId, user_id: userId },
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

export const removeOrganizationMember = async (
  organizationId: string,
  userId: string,
): Promise<Result<readonly OrganizationMember[]>> => {
  const response = await api("/orgs/{organization_id}/members/{user_id}", "delete", {
    path: { organization_id: organizationId, user_id: userId },
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
