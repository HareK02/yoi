import type { ApiResult } from "$lib/workspace/api/http";
import type {
  Diagnostic,
  GitCommitSummary,
  GitRemoteSummary,
  GitRepositorySummary,
  RepositoryDetailResponse,
  RepositoryDiagnostic,
  RepositoryListResponse,
  RepositoryLogResponse,
  RepositorySource,
  RepositorySourceKind,
  RepositorySummary,
  WorkspaceAuthConfig,
  WorkspaceCatalogListResponse,
  WorkspaceCreateResponse,
  WorkspaceExtensionPoints,
  WorkspaceExtensionPointState,
  WorkspacePermissionSummary,
  WorkspaceRepositoryRecord,
  WorkspaceResponse,
  WorkspaceSummary,
} from "$lib/generated/workspace-api.ts";

export type {
  GitCommitSummary,
  GitRemoteSummary,
  GitRepositorySummary,
  RepositoryDetailResponse,
  RepositoryListResponse,
  RepositoryLogResponse,
  RepositorySummary,
  WorkspaceCatalogListResponse,
  WorkspaceCreateResponse,
  WorkspacePermissionSummary,
  WorkspaceResponse,
  WorkspaceSummary,
} from "$lib/generated/workspace-api.ts";

type JsonObject = Record<string, unknown>;

const SOURCE_KINDS = new Set<RepositorySourceKind>([
  "local_path",
  "file",
  "ssh",
  "http",
  "https",
  "invalid",
]);
const OBSERVED_STATUSES = new Set(["unverified", "ready", "invalid"]);
const DIAGNOSTIC_SEVERITIES = new Set(["info", "warning", "error"]);

function object(value: unknown, path: string): JsonObject {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be an object`);
  }
  return value as JsonObject;
}

function array(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${path} must be an array`);
  return value;
}

function string(value: unknown, path: string): string {
  if (typeof value !== "string") throw new Error(`${path} must be a string`);
  return value;
}

function boolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${path} must be a boolean`);
  return value;
}

function integer(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new Error(`${path} must be a safe integer`);
  }
  return value;
}

function nullableString(value: unknown, path: string): string | null {
  return value === null ? null : string(value, path);
}

function optionalNullableString(
  value: unknown,
  path: string,
): string | null | undefined {
  return value === undefined ? undefined : nullableString(value, path);
}

function exactKeys(
  value: JsonObject,
  keys: readonly string[],
  path: string,
): void {
  const allowed = new Set(keys);
  const unexpected = Object.keys(value).find((key) => !allowed.has(key));
  if (unexpected) {
    throw new Error(`${path}.${unexpected} is not part of the wire contract`);
  }
}

function diagnostic(value: unknown, path: string): Diagnostic {
  const item = object(value, path);
  exactKeys(item, ["code", "severity", "message"], path);
  const severity = string(item.severity, `${path}.severity`);
  if (!DIAGNOSTIC_SEVERITIES.has(severity)) {
    throw new Error(`${path}.severity is invalid`);
  }
  return {
    code: string(item.code, `${path}.code`),
    severity: severity as Diagnostic["severity"],
    message: string(item.message, `${path}.message`),
  };
}

function repositoryDiagnostic(
  value: unknown,
  path: string,
): RepositoryDiagnostic {
  const item = object(value, path);
  exactKeys(item, ["severity", "code", "message"], path);
  return {
    severity: string(item.severity, `${path}.severity`),
    code: string(item.code, `${path}.code`),
    message: string(item.message, `${path}.message`),
  };
}

function repositorySource(value: unknown, path: string): RepositorySource {
  const source = object(value, path);
  exactKeys(source, ["kind", "uri"], path);
  const kind = string(source.kind, `${path}.kind`);
  if (!SOURCE_KINDS.has(kind as RepositorySourceKind)) {
    throw new Error(`${path}.kind is invalid`);
  }
  return {
    kind: kind as RepositorySourceKind,
    uri: string(source.uri, `${path}.uri`),
  };
}

function gitRemote(value: unknown, path: string): GitRemoteSummary {
  const remote = object(value, path);
  exactKeys(remote, ["name", "fetch_url"], path);
  return {
    name: string(remote.name, `${path}.name`),
    fetch_url: string(remote.fetch_url, `${path}.fetch_url`),
  };
}

function gitSummary(value: unknown, path: string): GitRepositorySummary {
  const git = object(value, path);
  exactKeys(git, ["status", "head", "branch", "dirty", "remotes"], path);
  return {
    status: string(git.status, `${path}.status`),
    head: nullableString(git.head, `${path}.head`),
    branch: nullableString(git.branch, `${path}.branch`),
    dirty: boolean(git.dirty, `${path}.dirty`),
    remotes: array(git.remotes, `${path}.remotes`).map((item, index) =>
      gitRemote(item, `${path}.remotes[${index}]`)
    ),
  };
}

function repositorySummary(value: unknown, path: string): RepositorySummary {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "repository_key",
      "kind",
      "provider",
      "source",
      "source_revision",
      "source_fingerprint",
      "observed_status",
      "observed_at",
      "default_selector",
      "record_authority",
      "git",
      "diagnostics",
    ],
    path,
  );
  const observedStatus = string(
    item.observed_status,
    `${path}.observed_status`,
  );
  if (!OBSERVED_STATUSES.has(observedStatus)) {
    throw new Error(`${path}.observed_status is invalid`);
  }
  const diagnostics =
    item.diagnostics === undefined || item.diagnostics === null
      ? item.diagnostics
      : array(item.diagnostics, `${path}.diagnostics`).map((entry, index) =>
        repositoryDiagnostic(entry, `${path}.diagnostics[${index}]`)
      );
  return {
    repository_key: string(item.repository_key, `${path}.repository_key`),
    kind: string(item.kind, `${path}.kind`),
    provider: string(item.provider, `${path}.provider`),
    source: repositorySource(item.source, `${path}.source`),
    source_revision: integer(item.source_revision, `${path}.source_revision`),
    source_fingerprint: string(
      item.source_fingerprint,
      `${path}.source_fingerprint`,
    ),
    observed_status: observedStatus as RepositorySummary["observed_status"],
    observed_at: optionalNullableString(
      item.observed_at,
      `${path}.observed_at`,
    ),
    default_selector: optionalNullableString(
      item.default_selector,
      `${path}.default_selector`,
    ),
    record_authority: string(item.record_authority, `${path}.record_authority`),
    git: item.git === undefined || item.git === null
      ? item.git
      : gitSummary(item.git, `${path}.git`),
    diagnostics,
  };
}

function workspaceSummary(value: unknown, path: string): WorkspaceSummary {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "workspace_id",
      "owner_account_id",
      "display_name",
      "state",
      "created_at",
      "updated_at",
    ],
    path,
  );
  return {
    workspace_id: string(item.workspace_id, `${path}.workspace_id`),
    owner_account_id: string(
      item.owner_account_id,
      `${path}.owner_account_id`,
    ),
    display_name: string(item.display_name, `${path}.display_name`),
    state: string(item.state, `${path}.state`),
    created_at: string(item.created_at, `${path}.created_at`),
    updated_at: string(item.updated_at, `${path}.updated_at`),
  };
}

function workspaceRepositoryRecord(
  value: unknown,
  path: string,
): WorkspaceRepositoryRecord {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "workspace_id",
      "repository_key",
      "kind",
      "provider",
      "source",
      "default_ref",
      "source_revision",
      "source_fingerprint",
      "observed_status",
      "observed_at",
      "created_at",
      "updated_at",
    ],
    path,
  );
  const observedStatus = string(
    item.observed_status,
    `${path}.observed_status`,
  );
  if (!OBSERVED_STATUSES.has(observedStatus)) {
    throw new Error(`${path}.observed_status is invalid`);
  }
  return {
    workspace_id: string(item.workspace_id, `${path}.workspace_id`),
    repository_key: string(item.repository_key, `${path}.repository_key`),
    kind: string(item.kind, `${path}.kind`),
    provider: nullableString(item.provider, `${path}.provider`),
    source: repositorySource(item.source, `${path}.source`),
    default_ref: nullableString(item.default_ref, `${path}.default_ref`),
    source_revision: integer(item.source_revision, `${path}.source_revision`),
    source_fingerprint: string(
      item.source_fingerprint,
      `${path}.source_fingerprint`,
    ),
    observed_status:
      observedStatus as WorkspaceRepositoryRecord["observed_status"],
    observed_at: nullableString(item.observed_at, `${path}.observed_at`),
    created_at: string(item.created_at, `${path}.created_at`),
    updated_at: string(item.updated_at, `${path}.updated_at`),
  };
}

function extensionPoint(
  value: unknown,
  path: string,
): WorkspaceExtensionPointState {
  const item = object(value, path);
  exactKeys(item, ["status", "note", "diagnostics"], path);
  return {
    status: string(item.status, `${path}.status`),
    note: string(item.note, `${path}.note`),
    diagnostics: array(item.diagnostics, `${path}.diagnostics`).map((
      entry,
      index,
    ) => diagnostic(entry, `${path}.diagnostics[${index}]`)),
  };
}

function extensionPoints(
  value: unknown,
  path: string,
): WorkspaceExtensionPoints {
  const item = object(value, path);
  exactKeys(item, [
    "store",
    "event_stream",
    "host_worker_bridge",
    "companion_console",
  ], path);
  return {
    store: string(item.store, `${path}.store`),
    event_stream: extensionPoint(item.event_stream, `${path}.event_stream`),
    host_worker_bridge: extensionPoint(
      item.host_worker_bridge,
      `${path}.host_worker_bridge`,
    ),
    companion_console: extensionPoint(
      item.companion_console,
      `${path}.companion_console`,
    ),
  };
}

function authConfig(value: unknown, path: string): WorkspaceAuthConfig {
  const auth = object(value, path);
  exactKeys(auth, ["Passkey"], path);
  const passkey = object(auth.Passkey, `${path}.Passkey`);
  exactKeys(
    passkey,
    ["rp_id", "origin", "public_base_url", "cookie_name"],
    `${path}.Passkey`,
  );
  return {
    Passkey: {
      rp_id: string(passkey.rp_id, `${path}.Passkey.rp_id`),
      origin: string(passkey.origin, `${path}.Passkey.origin`),
      public_base_url: string(
        passkey.public_base_url,
        `${path}.Passkey.public_base_url`,
      ),
      cookie_name: string(passkey.cookie_name, `${path}.Passkey.cookie_name`),
    },
  };
}

function permissions(value: unknown, path: string): WorkspacePermissionSummary {
  const item = object(value, path);
  exactKeys(item, ["manage_repositories", "manage_secrets"], path);
  return {
    manage_repositories: boolean(
      item.manage_repositories,
      `${path}.manage_repositories`,
    ),
    manage_secrets: boolean(item.manage_secrets, `${path}.manage_secrets`),
  };
}

function commitSummary(value: unknown, path: string): GitCommitSummary {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "hash",
      "short_hash",
      "summary",
      "author_name",
      "author_email",
      "author_date",
      "parents",
      "refs",
    ],
    path,
  );
  return {
    hash: string(item.hash, `${path}.hash`),
    short_hash: string(item.short_hash, `${path}.short_hash`),
    summary: string(item.summary, `${path}.summary`),
    author_name: string(item.author_name, `${path}.author_name`),
    author_email: string(item.author_email, `${path}.author_email`),
    author_date: string(item.author_date, `${path}.author_date`),
    parents: array(item.parents, `${path}.parents`).map((entry, index) =>
      string(entry, `${path}.parents[${index}]`)
    ),
    refs: array(item.refs, `${path}.refs`).map((entry, index) =>
      string(entry, `${path}.refs[${index}]`)
    ),
  };
}

export function parseWorkspaceCatalogResponse(
  value: unknown,
): WorkspaceCatalogListResponse {
  return array(value, "workspaces").map((item, index) =>
    workspaceSummary(item, `workspaces[${index}]`)
  );
}

export function parseWorkspaceCreateResponse(
  value: unknown,
): WorkspaceCreateResponse {
  const response = object(value, "workspace create response");
  exactKeys(
    response,
    [
      "workspace",
      "repository",
      "config_revision",
      "request_fingerprint",
      "replayed",
    ],
    "workspace create response",
  );
  return {
    workspace: workspaceSummary(
      response.workspace,
      "workspace create response.workspace",
    ),
    repository: workspaceRepositoryRecord(
      response.repository,
      "workspace create response.repository",
    ),
    config_revision: integer(
      response.config_revision,
      "workspace create response.config_revision",
    ),
    request_fingerprint: string(
      response.request_fingerprint,
      "workspace create response.request_fingerprint",
    ),
    replayed: boolean(response.replayed, "workspace create response.replayed"),
  };
}

export function parseWorkspaceResponse(value: unknown): WorkspaceResponse {
  const response = object(value, "workspace response");
  exactKeys(
    response,
    [
      "workspace_id",
      "display_name",
      "record_authority",
      "schema_version",
      "auth",
      "permissions",
      "extension_points",
    ],
    "workspace response",
  );
  return {
    workspace_id: string(
      response.workspace_id,
      "workspace response.workspace_id",
    ),
    display_name: string(
      response.display_name,
      "workspace response.display_name",
    ),
    record_authority: string(
      response.record_authority,
      "workspace response.record_authority",
    ),
    schema_version: integer(
      response.schema_version,
      "workspace response.schema_version",
    ),
    auth: authConfig(response.auth, "workspace response.auth"),
    permissions: permissions(
      response.permissions,
      "workspace response.permissions",
    ),
    extension_points: extensionPoints(
      response.extension_points,
      "workspace response.extension_points",
    ),
  };
}

export function parseRepositoryListResponse(
  value: unknown,
): RepositoryListResponse {
  const response = object(value, "repository list response");
  exactKeys(
    response,
    ["workspace_id", "items", "source", "diagnostics"],
    "repository list response",
  );
  return {
    workspace_id: string(
      response.workspace_id,
      "repository list response.workspace_id",
    ),
    items: array(response.items, "repository list response.items").map((
      item,
      index,
    ) => repositorySummary(item, `repository list response.items[${index}]`)),
    source: string(response.source, "repository list response.source"),
    diagnostics: array(
      response.diagnostics,
      "repository list response.diagnostics",
    ).map(
      (item, index) =>
        diagnostic(item, `repository list response.diagnostics[${index}]`),
    ),
  };
}

export function parseRepositoryListApiResult(
  result: ApiResult<unknown>,
): ApiResult<RepositoryListResponse> {
  if (result.data === null) return { data: null, error: result.error };
  try {
    return { data: parseRepositoryListResponse(result.data), error: null };
  } catch (cause) {
    return {
      data: null,
      error: cause instanceof Error
        ? cause.message
        : "invalid repository list response",
    };
  }
}

export function parseRepositoryDetailResponse(
  value: unknown,
): RepositoryDetailResponse {
  const response = object(value, "repository detail response");
  exactKeys(
    response,
    ["workspace_id", "item", "source"],
    "repository detail response",
  );
  return {
    workspace_id: string(
      response.workspace_id,
      "repository detail response.workspace_id",
    ),
    item: repositorySummary(response.item, "repository detail response.item"),
    source: string(response.source, "repository detail response.source"),
  };
}

export function parseRepositoryLogResponse(
  value: unknown,
): RepositoryLogResponse {
  const response = object(value, "repository log response");
  exactKeys(
    response,
    [
      "workspace_id",
      "repository_key",
      "default_selector",
      "limit",
      "items",
      "diagnostics",
    ],
    "repository log response",
  );
  return {
    workspace_id: string(
      response.workspace_id,
      "repository log response.workspace_id",
    ),
    repository_key: string(
      response.repository_key,
      "repository log response.repository_key",
    ),
    default_selector: optionalNullableString(
      response.default_selector,
      "repository log response.default_selector",
    ),
    limit: integer(response.limit, "repository log response.limit"),
    items: array(response.items, "repository log response.items").map((
      item,
      index,
    ) => commitSummary(item, `repository log response.items[${index}]`)),
    diagnostics: array(
      response.diagnostics,
      "repository log response.diagnostics",
    ).map(
      (item, index) =>
        diagnostic(item, `repository log response.diagnostics[${index}]`),
    ),
  };
}
