export type SettingsSectionId =
  | "runtime-connections"
  | "backend-config"
  | "workspace-identity";

export type SettingsSection = {
  readonly id: SettingsSectionId;
  readonly label: string;
  readonly status: "placeholder" | "read-only";
  readonly summary: string;
  readonly bullets: readonly string[];
};

export type SettingsPattern = {
  readonly title: string;
  readonly body: string;
};

export const SETTINGS_ROUTE = "/settings";

export const SETTINGS_PERMISSION_NOTICE =
  "Yoi currently has no browser user, role, permission, or multi-user authorization model. This shell is intentionally local and descriptive; it does not create an admin role or grant mutation authority.";

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  {
    id: "runtime-connections",
    label: "Runtime Connections",
    status: "placeholder",
    summary:
      "Future Runtime connection management will live here. The current view does not add, remove, test, or persist Runtime endpoints.",
    bullets: [
      "Shows where connection diagnostics will surface without exposing tokens, sockets, store roots, or raw endpoint secrets.",
      "Connection changes require a later typed Backend API and are not performed by this shell.",
      "Restart-required states should be shown as bounded diagnostics rather than live mutation controls.",
    ],
  },
  {
    id: "backend-config",
    label: "Backend Config",
    status: "placeholder",
    summary:
      "Configuration inspection is planned, but editing Backend config or secrets is out of scope for this shell.",
    bullets: [
      "Only sanitized summaries belong in the browser; raw config paths, secret refs, tokens, and store roots stay backend-side.",
      "Missing-provider or invalid-config states should be displayed as typed diagnostics.",
      "No fake permission model is created to make config editing appear available.",
    ],
  },
  {
    id: "workspace-identity",
    label: "Workspace Identity",
    status: "read-only",
    summary:
      "Workspace identity is presented as read-only context so operators can tell which workspace the browser is attached to.",
    bullets: [
      "Use opaque workspace ids and display names rather than raw filesystem paths.",
      "Repository/project-record authority remains backend-side and is not edited here.",
      "Identity changes need a later explicit migration flow.",
    ],
  },
];

export const SETTINGS_PATTERNS: readonly SettingsPattern[] = [
  {
    title: "Sanitized diagnostics",
    body:
      "Settings cards should show bounded codes and operator-facing messages, not raw socket paths, credentials, secret refs, token values, or Runtime store paths.",
  },
  {
    title: "Restart-required changes",
    body:
      "When a future setting cannot apply live, the browser should say restart required and leave the mutation to a typed Backend workflow.",
  },
  {
    title: "Read-only until typed APIs exist",
    body:
      "Placeholder sections describe planned surfaces without pretending that user, role, permission, or Runtime mutation APIs already exist.",
  },
];

export function settingsSectionHref(id: SettingsSectionId): string {
  return `${SETTINGS_ROUTE}#${id}`;
}
