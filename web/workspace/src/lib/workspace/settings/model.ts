export type Diagnostic = {
  severity: "info" | "warning" | "error";
  code: string;
  message: string;
};

export type SettingsSectionId =
  | "runtime-connections"
  | "runtime-inventory"
  | "configuration-sources"
  | "profile-sources"
  | "backend-config"
  | "workspace-identity";

export type SettingsSection = {
  readonly id: SettingsSectionId;
  readonly label: string;
  readonly status: "editable" | "placeholder" | "read-only";
  readonly summary: string;
  readonly bullets: readonly string[];
};

export type SettingsPattern = {
  readonly title: string;
  readonly body: string;
};

export type RuntimeConnectionSummary = {
  runtime_id: string;
  display_name: string;
  kind: string;
  built_in: boolean;
  config_managed: boolean;
  active: boolean;
  worker_creation_available: boolean;
  restart_required: boolean;
  status: string;
  diagnostics: Diagnostic[];
};

export type RemoteRuntimeConnectionSummary = RuntimeConnectionSummary & {
  endpoint_configured: boolean;
  token_ref_configured: boolean;
};

export type RuntimeConnectionSettingsResponse = {
  workspace_id: string;
  embedded: RuntimeConnectionSummary;
  remotes: RemoteRuntimeConnectionSummary[];
  diagnostics: Diagnostic[];
};

export type RuntimeConnectionMutationResponse = {
  workspace_id: string;
  restart_required: boolean;
  remotes: RemoteRuntimeConnectionSummary[];
  diagnostics: Diagnostic[];
};

export type RemoteRuntimeTestResponse = {
  workspace_id: string;
  runtime_id: string;
  checked_at: string;
  state: string;
  protocol_version?: string | null;
  compatibility_basis: string;
  capabilities: string[];
  health_result: string;
  diagnostics: Diagnostic[];
};

export const SETTINGS_ROUTE = "/settings";

export const SETTINGS_PERMISSION_NOTICE =
  "Yoi currently has no browser user, role, permission, or multi-user authorization model. This local settings surface uses typed Backend APIs only; it does not create an admin role or grant broad mutation authority.";

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  {
    id: "runtime-connections",
    label: "Runtime Connections",
    status: "editable",
    summary:
      "Manage remote Runtime connection records stored in the workspace-local Backend config. The embedded Runtime is built in and shown separately.",
    bullets: [
      "Remote connection changes are persisted through typed read-modify-write config updates and require a Backend restart before the live registry changes.",
      "The browser may submit a new endpoint, but Runtime endpoints, tokens, sockets, store roots, and config paths are not echoed back in API responses.",
      "Test negotiation is an observation only; checked_at, health, compatibility, and capability results are not persisted to local config.",
    ],
  },
  {
    id: "runtime-inventory",
    label: "Runtime Inventory",
    status: "read-only",
    summary:
      "Inspect registered Runtime handles and their materialized workdirs from the admin settings surface instead of the normal workspace navigation.",
    bullets: [
      "Runtime state is operational Backend/Runtime context, not a workspace content object like Objectives or Repositories.",
      "Workdir cleanup remains scoped to typed Runtime APIs and stays outside the primary workspace sidebar.",
      "Console routes may still target a Runtime handle directly, but Runtime discovery belongs under Settings.",
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
    id: "backend-config",
    label: "Backend Config",
    status: "placeholder",
    summary:
      "General Backend config editing remains out of scope; this page only exposes the Runtime Connections v0 typed surface.",
    bullets: [
      "Only sanitized summaries belong in the browser; raw config paths, secret refs, tokens, and store roots stay backend-side.",
      "Missing-provider or invalid-config states should be displayed as typed diagnostics.",
      "No fake permission model is created to make unrelated config editing appear available.",
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
    title: "Restart-required changes",
    body:
      "Remote Runtime config updates return restart_required=true because v0 does not unregister/register live Runtime handles.",
  },
  {
    title: "Typed Runtime surface only",
    body:
      "Runtime Connections v0 is intentionally narrow: embedded is built in, remote config is add/delete/test, and broader Backend admin controls stay unavailable.",
  },
];

export function settingsSectionHref(id: SettingsSectionId): string {
  switch (id) {
    case "runtime-connections":
      return `${SETTINGS_ROUTE}/runtime-connections`;
    case "runtime-inventory":
      return `${SETTINGS_ROUTE}/runtimes`;
    case "configuration-sources":
      return `${SETTINGS_ROUTE}/configuration`;
    case "profile-sources":
      return `${SETTINGS_ROUTE}/profiles`;
    case "workspace-identity":
      return `${SETTINGS_ROUTE}/workspace`;
    case "backend-config":
      return `${SETTINGS_ROUTE}/backend`;
  }
}

export function diagnosticLabel(diagnostic: Diagnostic): string {
  return `${diagnostic.severity}: ${diagnostic.code}`;
}
