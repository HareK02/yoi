/** Markdownのリンク先を明示的resource routeか外部HTTP(S)へ限定する。
 * Workspace contextがある場合、resource routeはそのWorkspaceだけを許可する。
 * URLはnavigation用であり、bearer credentialや自動fetch先ではない。
 * 任意の相対path、API path、protocol-relative URL、userinfo付き外部URLは拒否する。
 */
export const MARKDOWN_WORKSPACE_CONTEXT = "yoi-markdown-workspace";
export type MarkdownWorkspace = () => string;
export type MarkdownLinkTarget = { href: string; external: boolean };

export function markdownLinkTarget(
  value: string,
  workspaceId?: string,
): MarkdownLinkTarget | null {
  // Only explicit Workspace routes are accepted as relative links. Reject protocol-relative,
  // encoded separators, traversal, query strings and arbitrary app/API endpoints.
  if (value.startsWith("/w/") && !/[?#\\\s]/u.test(value)) {
    const match =
      /^\/w\/([^/]+)\/(drive|tickets|objectives|workers|memory)(?:\/([^/]+))?$/u
        .exec(value);
    if (!match) return null;
    try {
      const workspace = decodeURIComponent(match[1]);
      const reference = match[3] ? decodeURIComponent(match[3]) : "";
      if (
        !workspace || /[/\\\s]/u.test(workspace) || /[/\\]/u.test(reference) ||
        reference === "." || reference === ".."
      ) return null;
      if (workspaceId !== undefined && workspace !== workspaceId) return null;
      if (
        match[2] === "drive" && reference && !/^[1-9][0-9]*$/u.test(reference)
      ) return null;
      return { href: value, external: false };
    } catch {
      return null;
    }
  }
  try {
    const url = new URL(value);
    if (
      !["http:", "https:"].includes(url.protocol) || url.username ||
      url.password
    ) return null;
    return { href: value, external: true };
  } catch {
    return null;
  }
}
