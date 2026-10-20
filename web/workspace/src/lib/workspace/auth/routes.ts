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
    /%[0-7][0-9a-f]/i.test(rawPath) || rawPath.includes("//") ||
    rawPath.split("/").some((part) => part === "." || part === "..")
  ) {
    return AUTHENTICATED_HOME;
  }
  let url: URL;
  let decodedPath: string;
  try {
    decodedPath = decodeURIComponent(rawPath);
    // pathは一度だけdecodeして許可画面を判定する。UTF-8 escapeの大小や生Unicodeで
    // origin/区切りは変わらないため、再encodeとの文字列一致は求めない。
    // query/fragmentはpathの権限を持たないdata。単独の%25は保持し、次のdecodeで
    // 別文字へ変わる二重escapeとcontrol/backslashだけを拒否する。
    const decoded = decodeURIComponent(value);
    if (
      /[\\\u0000-\u001f\u007f]/u.test(decoded) || /%[0-9a-f]{2}/i.test(decoded)
    ) {
      return AUTHENTICATED_HOME;
    }
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
    ["/hosting/settings", "/hosting/accounts"].includes(url.pathname) ||
    /^\/w\/[a-zA-Z0-9_-]+(?:\/[\p{Letter}\p{Number}_/-]*)?$/u.test(decodedPath)
  ) {
    // 認証導線の入れ子は復帰先へ持ち込まない。検索条件とfragmentはURLとして保持する。
    if (url.searchParams.has("returnTo")) return url.pathname;
    return url.pathname + url.search + url.hash;
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

export function isPublicPage(path: string): boolean {
  return [
    "/",
    "/login",
    "/register",
    "/activate",
    "/login/device",
    "/hosting/setup",
  ]
    .includes(path);
}

export function isAuthPage(path: string): boolean {
  return ["/login", "/register", "/account", "/activate", "/login/device"]
    .includes(path);
}

// Universal loadのURLにfragmentがなくても同じ表示中URLならbrowserから補完する。
export function browserUrlSuffix(url: URL): string {
  const hash =
    typeof location !== "undefined" && location.pathname === url.pathname &&
      location.search === url.search
      ? location.hash
      : url.hash;
  return url.search + hash;
}
