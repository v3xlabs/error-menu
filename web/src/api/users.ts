import { api } from "./client";
import type { components } from "./schema.gen";

export type User = components["schemas"]["UserOutput"];
export type UserRole = components["schemas"]["UserRoleOutput"];
export type Result<Value>
  = | { ok: true; value: Value; }
    | { ok: false; reason: "unauthenticated" | "failed"; message: string; };

const failed = (status: number): Result<never> => ({
  ok: false,
  reason: "failed",
  message: `Request failed with status ${status}.`,
});

export const currentUser = async (): Promise<Result<User>> => {
  const response = await api("/user", "get", {});

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 401: {
      return { ok: false, reason: "unauthenticated", message: response.data.message };
    }
    case 403:
    case 404:
    case 500: {
      return { ok: false, reason: "failed", message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const listUsers = async (): Promise<Result<readonly User[]>> => {
  const response = await api("/users", "get", {});

  if (response.status === 200) return { ok: true, value: response.data.users };

  switch (response.status) {
    case 401: {
      return { ok: false, reason: "unauthenticated", message: response.data.message };
    }
    case 403:
    case 500: {
      return { ok: false, reason: "failed", message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

export const setUserRole = async (userId: string, role: UserRole): Promise<Result<User>> => {
  const response = await api("/users/{user_id}/role", "put", {
    path: { user_id: userId },
    contentType: "application/json; charset=utf-8",
    data: { role },
  });

  if (response.status === 200) return { ok: true, value: response.data };

  switch (response.status) {
    case 401: {
      return { ok: false, reason: "unauthenticated", message: response.data.message };
    }
    case 400:
    case 403:
    case 404:
    case 500: {
      return { ok: false, reason: "failed", message: response.data.message };
    }
    default: {
      return failed(response.status);
    }
  }
};

// The session cookie is cleared by the server, and `/auth/logout` is outside the OpenAPI
// document, so this one call cannot go through the generated client.
export const logout = async (): Promise<Result<void>> => {
  const response = await fetch("/auth/logout", { method: "POST" });

  if (response.status === 204) return { ok: true, value: undefined };

  return failed(response.status);
};
