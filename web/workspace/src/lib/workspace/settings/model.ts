export type Diagnostic = {
  severity: "info" | "warning" | "error";
  code: string;
  message: string;
};

export type SettingsSectionId =
  | "runtimes"
  | "configuration-sources"
  | "repository-access"
  | "profile-sources"
  | "workspace-identity";

export type SettingsSection = {
  readonly id: SettingsSectionId;
  readonly label: string;
  readonly status: "editable" | "read-only";
  readonly summary: string;
  readonly bullets: readonly string[];
};

export type SettingsPattern = {
  readonly title: string;
  readonly body: string;
};

export const SETTINGS_ROUTE = "/settings";

export const SETTINGS_PERMISSION_NOTICE =
  "Workspace settings use authenticated account authority and Workspace-scoped typed Backend APIs. Repository secret management requires the current Workspace owner; this surface does not expose secret material or grant Runtime execution authority.";

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  {
    id: "runtimes",
    label: "Runtimes",
    status: "editable",
    summary:
      "Register and inspect Workspace Runtimes, verify connectivity, and open their Workdir inventory from one resource list.",
    bullets: [
      "Embedded and remote Runtimes share one canonical Workspace resource representation.",
      "Remote Runtime creation, connection tests, and guarded deletion use the same REST collection.",
      "Runtime status, worker creation availability, diagnostics, and Workdir inventory remain visible without exposing endpoints or credentials.",
    ],
  },
  {
    id: "configuration-sources",
    label: "Configuration Sources",
    status: "editable",
    summary:
      "Edit the Server-owned virtual Decodal source tree through one native/WASM toolchain contract.",
    bullets: [
      "Virtual paths and imports resolve inside the committed Workspace tree, never from browser or Server host paths.",
      "Browser analysis is advisory; Server evaluation is required before an atomic revision commit.",
      "Profile launch data is projected from this active revision; remaining Skill, Prompt, and Plugin consumers migrate in their follow-up cutovers.",
    ],
  },
  {
    id: "repository-access",
    label: "Repository Access",
    status: "editable",
    summary:
      "Manage Workspace-scoped SSH credentials and pinned host keys without exposing stored secret material.",
    bullets: [
      "Private keys and passphrases are write-only; list and detail responses contain public metadata only.",
      "Host trust requires an explicitly pinned key and never uses accept-new or TOFU.",
      "Repository bindings are committed through the shared Workspace configuration editor and validated against these records.",
    ],
  },
  {
    id: "profile-sources",
    label: "Profile Sources",
    status: "editable",
    summary:
      "Inspect the Profile launch projection derived from the active Workspace configuration revision.",
    bullets: [
      "Selectors are source-qualified (builtin:* or project:*); raw profile source paths, archive content, archive digests, resource handles, and runtime tokens are not exposed.",
      "Profile declarations and sources are edited only through the shared Workspace configuration editor and evaluate-before-commit contract.",
      "Launch candidates and Profile archives carry the same active config revision, tree digest, and projection digest.",
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
      "Settings cards show bounded codes and operator-facing messages, not raw socket paths, credentials, token values, Runtime endpoints, or Runtime store paths.",
  },
  {
    title: "Live Runtime resources",
    body:
      "Remote Runtime create/delete operations update persisted configuration and the live Runtime registry through one canonical REST resource.",
  },
  {
    title: "Typed Runtime surface only",
    body:
      "The Runtime REST resource exposes embedded and remote inventory, guarded create/delete/test operations, and Workdir links without broader Backend admin controls.",
  },
];

export function settingsSectionHref(id: SettingsSectionId): string {
  switch (id) {
    case "runtimes":
      return `${SETTINGS_ROUTE}/runtimes`;
    case "configuration-sources":
      return `${SETTINGS_ROUTE}/configuration`;
    case "repository-access":
      return `${SETTINGS_ROUTE}/repository-access`;
    case "profile-sources":
      return `${SETTINGS_ROUTE}/profiles`;
    case "workspace-identity":
      return `${SETTINGS_ROUTE}/workspace`;
  }
}

export function diagnosticLabel(diagnostic: Diagnostic): string {
  return `${diagnostic.severity}: ${diagnostic.code}`;
}
