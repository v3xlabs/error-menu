const REMOTE_URL_SCHEMES = ["https://", "ssh://"] as const;

// The same two transports the backend allows, checked here so the dialog can say no before
// a request does. `RemoteUrl::new` is still the authority.
export const isValidRemoteUrl = (remoteUrl: string): boolean =>
  REMOTE_URL_SCHEMES.some(scheme => remoteUrl.startsWith(scheme)) && remoteUrl.length > REMOTE_URL_SCHEMES[0].length;

// Host and path read the same on every forge; the transport and a trailing `.git` add nothing.
export const remoteLocation = (remoteUrl: string): string =>
  remoteUrl.replace(/^[a-z]+:\/\//, "").replace(/^[^@/]+@/, "")
    .replace(/\.git$/, "");

// The last path segment of any remote, `https://host/acme/Example.git` or
// `ssh://git@host:acme/example`, lowercased for a display name the owner can still change.
export const repoName = (remoteUrl: string): string =>
  remoteUrl.trim().replace(/\/+$/, "")
    .replace(/\.git$/, "")
    .split(/[/:]/)
    .at(-1)
    ?.toLowerCase() ?? "";
