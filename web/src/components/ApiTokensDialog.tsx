import { Dialog } from "@kobalte/core/dialog";
import { createSignal, For, Match, Show, Switch } from "solid-js";

import type { ApiToken } from "../api/tokens";
import { createApiToken, listApiTokens, revokeApiToken } from "../api/tokens";

type TokensState
  = | { phase: "idle"; }
    | { phase: "loading"; }
    | { phase: "loaded"; tokens: readonly ApiToken[]; }
    | { phase: "error"; message: string; };
type WriteState
  = | { phase: "ready"; }
    | { phase: "creating"; }
    | { phase: "revoking"; name: string; }
    | { phase: "error"; message: string; };
type CopyState = "ready" | "copying" | "copied" | "error";
type Expiry = "ninety-days" | "one-year" | "never";

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export const ApiTokensDialog = (properties: { open: boolean; onOpenChange: (isOpen: boolean) => void; }) => {
  const [tokensState, setTokensState] = createSignal<TokensState>({ phase: "idle" });
  const [writeState, setWriteState] = createSignal<WriteState>({ phase: "ready" });
  const [name, setName] = createSignal("");
  const [expiry, setExpiry] = createSignal<Expiry>("ninety-days");
  const [rawToken, setRawToken] = createSignal<string | null>(null);
  const [copyState, setCopyState] = createSignal<CopyState>("ready");
  const writeError = (): string | undefined => {
    const current = writeState();

    if (current.phase === "error") return current.message;

    return undefined;
  };

  const tokensError = (): string | undefined => {
    const current = tokensState();

    if (current.phase === "error") return current.message;

    return undefined;
  };

  const tokens = (): readonly ApiToken[] | undefined => {
    const current = tokensState();

    if (current.phase === "loaded") return current.tokens;

    return undefined;
  };

  const revokingTokenName = (): string | undefined => {
    const current = writeState();

    if (current.phase === "revoking") return current.name;

    return undefined;
  };

  const loadTokens = async (): Promise<void> => {
    setTokensState({ phase: "loading" });

    const result = await listApiTokens();

    setTokensState(result.ok
      ? { phase: "loaded", tokens: result.value }
      : { phase: "error", message: result.message });
  };

  const handleOpenChange = (isNowOpen: boolean): void => {
    properties.onOpenChange(isNowOpen);

    if (isNowOpen) {
      void loadTokens();

      return;
    }

    setRawToken(null);
    setCopyState("ready");
  };

  const createToken = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();

    const tokenName = name().trim();

    if (tokenName.length === 0) {
      setWriteState({ phase: "error", message: "Enter a token name." });

      return;
    }

    const expiryValue = expiry();
    const expiresAt = new Date();

    if (expiryValue === "ninety-days") {
      expiresAt.setUTCDate(expiresAt.getUTCDate() + 90);
    }
    else if (expiryValue === "one-year") {
      expiresAt.setUTCFullYear(expiresAt.getUTCFullYear() + 1);
    }

    setWriteState({ phase: "creating" });
    const result = await createApiToken({
      name: tokenName,
      ...(expiryValue !== "never" && { expires_at: expiresAt.toISOString() }),
    });

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return;
    }

    setTokensState(current => (current.phase === "loaded"
      ? { phase: "loaded", tokens: [result.value, ...current.tokens] }
      : { phase: "loaded", tokens: [result.value] }));
    setRawToken(result.value.token);
    setCopyState("ready");
    setName("");
    setWriteState({ phase: "ready" });
  };

  const copyToken = async (): Promise<void> => {
    const value = rawToken();

    if (value === null) return;

    setCopyState("copying");

    try {
      await navigator.clipboard.writeText(value);
      setCopyState("copied");
    }
    catch {
      setCopyState("error");
    }
  };

  const revokeToken = async (tokenName: string): Promise<void> => {
    setWriteState({ phase: "revoking", name: tokenName });

    const result = await revokeApiToken(tokenName);

    if (!result.ok) {
      setWriteState({ phase: "error", message: result.message });

      return;
    }

    setTokensState(current => (current.phase === "loaded"
      ? { phase: "loaded", tokens: current.tokens.filter(token => token.name !== tokenName) }
      : current));
    setWriteState({ phase: "ready" });
  };

  return (
    <Dialog open={properties.open} onOpenChange={handleOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay class="fixed inset-0 z-40 bg-slate-950/40" />
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
          <Dialog.Content class="max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-panel bg-surface p-5 shadow-xl">
            <Dialog.Title class="text-base font-semibold text-slate-900 dark:text-slate-100">API tokens</Dialog.Title>
            <p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
              Create a token for read-only API access. The token is shown only after creation.
            </p>
            <a href="/docs" class="mt-2 inline-block text-sm font-medium text-slate-700 underline hover:text-slate-950 dark:text-slate-300 dark:hover:text-white">
              Read API documentation
            </a>
            <Show when={writeError()}>
              {message => <p class="mt-4 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
            </Show>
            <Show when={rawToken()}>
              {token => (
                <section class="mt-4 rounded-panel bg-amber-50 p-3 dark:bg-amber-950/30">
                  <h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">Copy your new token</h2>
                  <p class="mt-1 text-sm text-slate-700 dark:text-slate-300">It will not be shown again after you close this dialog.</p>
                  <textarea
                    aria-label="New API token"
                    readonly
                    value={token()}
                    rows={3}
                    class="mt-3 w-full resize-none rounded-control bg-surface p-2 font-mono text-xs text-slate-900 dark:text-slate-100"
                  />
                  <div class="mt-2 flex items-center gap-3">
                    <button
                      type="button"
                      disabled={copyState() === "copying"}
                      onClick={() => void copyToken()}
                      class="rounded-control px-3 py-1.5 text-sm font-medium text-slate-800 hover:bg-amber-100 disabled:cursor-not-allowed disabled:opacity-60 dark:text-amber-100 dark:hover:bg-amber-900"
                    >
                      <Switch>
                        <Match when={copyState() === "copying"}>Copying...</Match>
                        <Match when={copyState() === "copied"}>Copied</Match>
                        <Match when={copyState() === "error"}>Copy failed</Match>
                        <Match when={true}>Copy token</Match>
                      </Switch>
                    </button>
                    <Show when={copyState() === "error"}>
                      <span class="text-sm text-red-600 dark:text-red-400" role="alert">Select the token and copy it manually.</span>
                    </Show>
                  </div>
                </section>
              )}
            </Show>
            <form class="mt-5 space-y-3" onSubmit={event => void createToken(event)}>
              <label class="block">
                <span class="text-sm font-medium text-slate-700 dark:text-slate-300">Token name</span>
                <input
                  type="text"
                  required
                  maxlength={128}
                  value={name()}
                  disabled={writeState().phase === "creating" || writeState().phase === "revoking"}
                  onInput={event => setName(event.currentTarget.value)}
                  class="mt-1 w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-100"
                />
              </label>
              <label class="block">
                <span class="text-sm font-medium text-slate-700 dark:text-slate-300">Expiry</span>
                <select
                  value={expiry()}
                  disabled={writeState().phase === "creating" || writeState().phase === "revoking"}
                  onChange={(event) => {
                    const value = event.currentTarget.value;

                    switch (value) {
                      case "ninety-days":
                      case "one-year":
                      case "never": {
                        setExpiry(value);
                      }
                    }
                  }}
                  class="mt-1 w-full rounded-control bg-raised px-3 py-1.5 text-sm text-slate-900 disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-100"
                >
                  <option value="ninety-days">90 days</option>
                  <option value="one-year">One year</option>
                  <option value="never">No expiry</option>
                </select>
              </label>
              <button
                type="submit"
                disabled={writeState().phase === "creating" || writeState().phase === "revoking"}
                class="rounded-control bg-slate-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-slate-100 dark:text-slate-900 dark:hover:bg-white"
              >
                {writeState().phase === "creating" ? "Creating..." : "Create token"}
              </button>
            </form>
            <section class="mt-6">
              <h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">Active tokens</h2>
              <Show when={tokensState().phase === "loading"}>
                <p class="mt-3 text-sm text-slate-500 dark:text-slate-400" role="status">Loading tokens...</p>
              </Show>
              <Show when={tokensError()}>
                {message => <p class="mt-3 text-sm text-red-600 dark:text-red-400" role="alert">{message()}</p>}
              </Show>
              <Show when={tokens()}>
                {tokens => (
                  <ul class="mt-3 divide-y divide-hairline">
                    <For each={tokens()} fallback={<li class="px-1 py-6 text-center text-sm text-slate-500 dark:text-slate-400">No API tokens yet.</li>}>
                      {token => (
                        <li class="flex items-center justify-between gap-4 px-1 py-3">
                          <div class="min-w-0">
                            <p class="truncate text-sm font-medium text-slate-900 dark:text-slate-100">{token.name}</p>
                            <p class="text-xs text-slate-500 dark:text-slate-400">
                              {`Created ${dateFormatter.format(new Date(token.created_at))} - ${token.expires_at === undefined ? "No expiry" : `Expires ${dateFormatter.format(new Date(token.expires_at))}`}`}
                            </p>
                          </div>
                          <button
                            type="button"
                            disabled={writeState().phase === "creating" || writeState().phase === "revoking"}
                            onClick={() => void revokeToken(token.name)}
                            class="shrink-0 rounded-control bg-raised px-2 py-1 text-sm font-medium text-slate-700 hover:bg-raised-hover disabled:cursor-not-allowed disabled:opacity-60 dark:text-slate-300"
                          >
                            {revokingTokenName() === token.name ? "Revoking..." : "Revoke"}
                          </button>
                        </li>
                      )}
                    </For>
                  </ul>
                )}
              </Show>
            </section>
            <div class="mt-5 flex justify-end">
              <Dialog.CloseButton class="rounded-control bg-raised px-3 py-1.5 text-sm font-medium text-slate-700 hover:bg-raised-hover dark:text-slate-300">
                Close
              </Dialog.CloseButton>
            </div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
};
