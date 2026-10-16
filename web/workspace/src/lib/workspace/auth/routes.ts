// 公開トップではなく明示的なWorkspace選択へ戻す。先頭Workspaceの推測はしない。
// 戻り先は既知のアプリ画面だけ。認証経路への再帰やdecode段階の差による別originを作らない。
export const AUTHENTICATED_HOME = "/dashboard";

export function safeReturnTo(value: string | null): string {
  if (
    !value || value.length > 2048 || !value.startsWith("/") ||
    /[\\\s\u0000-\u001f\u007f]/u.test(value) || value.startsWith("//")
  ) {
    return AUTHENTICATED_HOME;
  }
  const rawPath = value.split(/[?#]/, 1)[0];
  if (
    rawPath.includes("%") || rawPath.includes("//") ||
    rawPath.split("/").some((part) => part === "." || part === "..")
  ) {
    return AUTHENTICATED_HOME;
  }
  let url: URL;
  try {
    url = new URL(value, "https://app.invalid");
  } catch {
    return AUTHENTICATED_HOME;
  }
  if (url.origin !== "https://app.invalid") return AUTHENTICATED_HOME;
  if (url.pathname === "/login/device") {
    const code = url.searchParams.get("user_code") ??
      url.searchParams.get("code");
    if (code && /^[a-zA-Z0-9-]{1,64}$/.test(code)) {
      return `/login/device?user_code=${encodeURIComponent(code)}`;
    }
    return "/login/device";
  }
  if (
    url.pathname === AUTHENTICATED_HOME || url.pathname === "/account" ||
    /^\/w\/[a-zA-Z0-9_-]+(?:\/[a-zA-Z0-9_/-]*)?$/.test(url.pathname)
  ) {
    return url.pathname;
  }
  return AUTHENTICATED_HOME;
}

export function loginUrl(returnTo: string): string {
  return `/login?${new URLSearchParams({ returnTo: safeReturnTo(returnTo) })}`;
}

export function authEntryUrl(
  route: "/login" | "/register",
  returnTo: string,
): string {
  return `${route}?${new URLSearchParams({
    returnTo: safeReturnTo(returnTo),
  })}`;
}

export function isAuthPage(path: string): boolean {
  return ["/login", "/register", "/account", "/activate", "/login/device"]
    .includes(path);
}
