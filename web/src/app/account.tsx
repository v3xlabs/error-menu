import type { JSX } from "@solidjs/web";
import { createContext, createEffect, createSignal, useContext } from "solid-js";

import type { User } from "../api/users";
import { currentUser } from "../api/users";

type AccountState
  = | { phase: "loading"; }
    | { phase: "anonymous"; }
    | { phase: "loaded"; user: User; }
    | { phase: "error"; message: string; };

type Account = {
  state: () => AccountState;
  user: () => User | undefined;
  reload: () => Promise<void>;
};

const AccountContext = createContext<Account>();

export const AccountProvider = (properties: { children?: JSX.Element; }) => {
  const [state, setState] = createSignal<AccountState>({ phase: "loading" });

  const reload = async (): Promise<void> => {
    setState({ phase: "loading" });

    const result = await currentUser();

    if (result.ok) {
      setState({ phase: "loaded", user: result.value });

      return;
    }

    setState(result.reason === "unauthenticated"
      ? { phase: "anonymous" }
      : { phase: "error", message: result.message });
  };

  const user = (): User | undefined => {
    const current = state();

    return current.phase === "loaded" ? current.user : undefined;
  };

  createEffect(
    () => undefined,
    () => {
      void reload();
    },
  );

  return <AccountContext value={{ state, user, reload }}>{properties.children}</AccountContext>;
};

export const useAccount = (): Account => useContext(AccountContext);
