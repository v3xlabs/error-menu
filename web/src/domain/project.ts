const REMOTE_URL_SCHEMES = ["https://", "ssh://"] as const;

// The same two transports the backend allows, checked here so the dialog can say no before
// a request does. `RemoteUrl::new` is still the authority.
export const isValidRemoteUrl = (remoteUrl: string): boolean =>
  REMOTE_URL_SCHEMES.some(scheme => remoteUrl.startsWith(scheme)) && remoteUrl.length > REMOTE_URL_SCHEMES[0].length;
