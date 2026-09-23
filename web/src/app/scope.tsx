import type { JSX } from "@solidjs/web";
import { createContext, createSignal, useContext } from "solid-js";

const SCOPE_STORAGE_KEY = "error-menu-organization-scope";

type OrganizationScope = {
  organizationId: () => string | null;
  choose: (organizationId: string | null) => void;
};

const ScopeContext = createContext<OrganizationScope>();

export const ScopeProvider = (properties: { children?: JSX.Element; }) => {
  const [organizationId, setOrganizationId] = createSignal<string | null>(localStorage.getItem(SCOPE_STORAGE_KEY));

  const choose = (next: string | null): void => {
    setOrganizationId(next);

    if (next === null) localStorage.removeItem(SCOPE_STORAGE_KEY);
    else localStorage.setItem(SCOPE_STORAGE_KEY, next);
  };

  return <ScopeContext value={{ organizationId, choose }}>{properties.children}</ScopeContext>;
};

export const useScope = (): OrganizationScope => useContext(ScopeContext);
