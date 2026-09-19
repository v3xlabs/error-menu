import { api } from "./client";
import type { components } from "./schema.gen";

export type ApiToken = components["schemas"]["ApiTokenOutput"];
type CreateApiToken = components["schemas"]["CreateApiToken"];
export type CreatedApiToken = components["schemas"]["CreatedApiTokenOutput"];

type Result<Value> = { ok: true; value: Value; } | { ok: false; message: string; };

export const listApiTokens = async (): Promise<Result<readonly ApiToken[]>> => {
  const response = await api("/tokens", "get", {});

  if (response.status === 200) return { ok: true, value: response.data.tokens };

  switch (response.status) {
    case 400:
    case 401:
    case 403:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return { ok: false, message: `Request failed with status ${response.status}.` };
    }
  }
};

export const createApiToken = async (input: CreateApiToken): Promise<Result<CreatedApiToken>> => {
  const response = await api("/tokens", "post", {
    contentType: "application/json; charset=utf-8",
    data: input,
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
      return { ok: false, message: `Request failed with status ${response.status}.` };
    }
  }
};

export const revokeApiToken = async (name: string): Promise<Result<void>> => {
  const response = await api("/tokens/{name}", "delete", { path: { name } });

  if (response.status === 204) return { ok: true, value: undefined };

  switch (response.status) {
    case 401:
    case 403:
    case 404:
    case 500: {
      return { ok: false, message: response.data.message };
    }
    default: {
      return { ok: false, message: `Request failed with status ${response.status}.` };
    }
  }
};
