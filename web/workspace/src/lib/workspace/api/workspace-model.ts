import type { ApiResult } from "$lib/workspace/api/http";
import type {
  WorkspaceAuthConfig,
  WorkspaceCatalogListResponse,
  WorkspaceCreateResponse,
  WorkspaceDeletionBlocker,
  WorkspaceDeletionBlockerKind,
  WorkspaceDeletionOperationResponse,
  WorkspaceDeletionPreflightResponse,
  WorkspaceDeletionResourceCounts,
  WorkspaceDeletionState,
  WorkspaceExtensionPoints,
  WorkspaceExtensionPointState,
  WorkspacePermissionSummary,
  WorkspaceRepositoryRecord,
  WorkspaceResponse,
  WorkspaceSummary,
} from "$lib/generated/legacy-server-api.ts";
import type {
  CreateWorkspaceRepositoryResponse,
  Diagnostic,
  GitCommitSummary,
  GitRemoteSummary,
  GitRepositorySummary,
  HostListResponse,
  HostSummary,
  RepositoryApiError,
  RepositoryDetailResponse,
  RepositoryDiagnostic,
  RepositoryListResponse,
  RepositoryLogResponse,
  RepositorySource,
  RepositorySourceKind,
  RepositorySshConnectionProbeResponse,
  RepositorySshConnectionTrustState,
  RepositorySshHostKeyCandidate,
  RepositorySummary,
} from "$lib/generated/repository-api.ts";

export type {
  WorkspaceCatalogListResponse,
  WorkspaceCreateResponse,
  WorkspaceDeletionOperationResponse,
  WorkspaceDeletionPreflightResponse,
  WorkspacePermissionSummary,
  WorkspaceResponse,
  WorkspaceSummary,
} from "$lib/generated/legacy-server-api.ts";
export type {
  CreateWorkspaceRepositoryRequest,
  CreateWorkspaceRepositoryResponse,
  GitCommitSummary,
  GitRemoteSummary,
  GitRepositorySummary,
  HostListResponse,
  HostSummary,
  RepositoryApiError,
  RepositoryDetailResponse,
  RepositoryListResponse,
  RepositoryLogResponse,
  RepositorySource,
  RepositorySourceKind,
  RepositorySshConnectionProbeResponse,
  RepositorySshHostKeyCandidate,
  RepositorySummary,
} from "$lib/generated/repository-api.ts";

type JsonObject = Record<string, unknown>;

const SOURCE_KINDS = new Set<RepositorySourceKind>([
  "local_path",
  "file",
  "ssh",
  "https",
  "invalid",
]);
const OBSERVED_STATUSES = new Set(["unverified", "ready", "invalid"]);
const DIAGNOSTIC_SEVERITIES = new Set(["info", "warning", "error"]);

export const REPOSITORY_API_LIMITS = {
  maxStringCodeUnits: 65_536,
  maxCollectionEntries: 4_096,
} as const;

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

function repositoryString(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (parsed.length > REPOSITORY_API_LIMITS.maxStringCodeUnits) {
    throw new Error(`${path} exceeds the Repository API string limit`);
  }
  return parsed;
}

function nullableRepositoryString(value: unknown, path: string): string | null {
  return value === null ? null : repositoryString(value, path);
}

function repositoryArray(value: unknown, path: string): unknown[] {
  const parsed = array(value, path);
  if (parsed.length > REPOSITORY_API_LIMITS.maxCollectionEntries) {
    throw new Error(`${path} exceeds the Repository API collection limit`);
  }
  return parsed;
}

function repositorySourceRevision(value: unknown, path: string): number {
  const parsed = integer(value, path);
  if (parsed < 0) {
    throw new Error(`${path} must be between 0 and Number.MAX_SAFE_INTEGER`);
  }
  return parsed;
}

function optionalNullableString(
  value: unknown,
  path: string,
): string | null | undefined {
  return value === undefined ? undefined : nullableString(value, path);
}

function optionalNullableRepositoryString(
  value: unknown,
  path: string,
): string | null | undefined {
  return value === undefined
    ? undefined
    : nullableRepositoryString(value, path);
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

function repositoryApiDiagnostic(value: unknown, path: string): Diagnostic {
  const item = object(value, path);
  exactKeys(item, ["code", "severity", "message"], path);
  const severity = repositoryString(item.severity, `${path}.severity`);
  if (!DIAGNOSTIC_SEVERITIES.has(severity)) {
    throw new Error(`${path}.severity is invalid`);
  }
  return {
    code: repositoryString(item.code, `${path}.code`),
    severity: severity as Diagnostic["severity"],
    message: repositoryString(item.message, `${path}.message`),
  };
}

function repositoryDiagnostic(
  value: unknown,
  path: string,
): RepositoryDiagnostic {
  const item = object(value, path);
  exactKeys(item, ["severity", "code", "message"], path);
  return {
    severity: repositoryString(item.severity, `${path}.severity`),
    code: repositoryString(item.code, `${path}.code`),
    message: repositoryString(item.message, `${path}.message`),
  };
}

function repositorySource(value: unknown, path: string): RepositorySource {
  const source = object(value, path);
  exactKeys(source, ["kind", "uri"], path);
  const kind = repositoryString(source.kind, `${path}.kind`);
  if (!SOURCE_KINDS.has(kind as RepositorySourceKind)) {
    throw new Error(`${path}.kind is invalid`);
  }
  return {
    kind: kind as RepositorySourceKind,
    uri: repositoryString(source.uri, `${path}.uri`),
  };
}

function gitRemote(value: unknown, path: string): GitRemoteSummary {
  const remote = object(value, path);
  exactKeys(remote, ["name", "fetch_url"], path);
  return {
    name: repositoryString(remote.name, `${path}.name`),
    fetch_url: repositoryString(remote.fetch_url, `${path}.fetch_url`),
  };
}

function gitSummary(value: unknown, path: string): GitRepositorySummary {
  const git = object(value, path);
  exactKeys(git, ["status", "head", "branch", "dirty", "remotes"], path);
  return {
    status: repositoryString(git.status, `${path}.status`),
    head: optionalNullableRepositoryString(git.head, `${path}.head`),
    branch: optionalNullableRepositoryString(git.branch, `${path}.branch`),
    dirty: boolean(git.dirty, `${path}.dirty`),
    remotes: repositoryArray(git.remotes, `${path}.remotes`).map((
      item,
      index,
    ) => gitRemote(item, `${path}.remotes[${index}]`)),
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
  const observedStatus = repositoryString(
    item.observed_status,
    `${path}.observed_status`,
  );
  if (!OBSERVED_STATUSES.has(observedStatus)) {
    throw new Error(`${path}.observed_status is invalid`);
  }
  const diagnostics =
    item.diagnostics === undefined || item.diagnostics === null
      ? item.diagnostics
      : repositoryArray(item.diagnostics, `${path}.diagnostics`).map((
        entry,
        index,
      ) => repositoryDiagnostic(entry, `${path}.diagnostics[${index}]`));
  return {
    repository_key: repositoryString(
      item.repository_key,
      `${path}.repository_key`,
    ),
    kind: repositoryString(item.kind, `${path}.kind`),
    provider: repositoryString(item.provider, `${path}.provider`),
    source: repositorySource(item.source, `${path}.source`),
    source_revision: repositorySourceRevision(
      item.source_revision,
      `${path}.source_revision`,
    ),
    source_fingerprint: repositoryString(
      item.source_fingerprint,
      `${path}.source_fingerprint`,
    ),
    observed_status: observedStatus as RepositorySummary["observed_status"],
    observed_at: optionalNullableRepositoryString(
      item.observed_at,
      `${path}.observed_at`,
    ),
    default_selector: optionalNullableRepositoryString(
      item.default_selector,
      `${path}.default_selector`,
    ),
    record_authority: repositoryString(
      item.record_authority,
      `${path}.record_authority`,
    ),
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
  const observedStatus = repositoryString(
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
    source_revision: repositorySourceRevision(
      item.source_revision,
      `${path}.source_revision`,
    ),
    source_fingerprint: repositoryString(
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

function workspaceString(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (parsed.length > REPOSITORY_API_LIMITS.maxStringCodeUnits) {
    throw new Error(`${path} exceeds the Workspace API string limit`);
  }
  return parsed;
}

function workspaceDiagnostics(value: unknown, path: string): Diagnostic[] {
  const entries = array(value, path);
  if (entries.length > 100) {
    throw new Error(`${path} exceeds the Workspace API collection limit`);
  }
  return entries.map((entry, index) => {
    const parsed = diagnostic(entry, `${path}[${index}]`);
    workspaceString(parsed.code, `${path}[${index}].code`);
    workspaceString(parsed.message, `${path}[${index}].message`);
    return parsed;
  });
}

function extensionPoint(
  value: unknown,
  path: string,
): WorkspaceExtensionPointState {
  const item = object(value, path);
  exactKeys(item, ["status", "note", "diagnostics"], path);
  return {
    status: workspaceString(item.status, `${path}.status`),
    note: workspaceString(item.note, `${path}.note`),
    diagnostics: workspaceDiagnostics(item.diagnostics, `${path}.diagnostics`),
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
    store: workspaceString(item.store, `${path}.store`),
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
      rp_id: workspaceString(passkey.rp_id, `${path}.Passkey.rp_id`),
      origin: workspaceString(passkey.origin, `${path}.Passkey.origin`),
      public_base_url: workspaceString(
        passkey.public_base_url,
        `${path}.Passkey.public_base_url`,
      ),
      cookie_name: workspaceString(
        passkey.cookie_name,
        `${path}.Passkey.cookie_name`,
      ),
    },
  };
}

function permissions(value: unknown, path: string): WorkspacePermissionSummary {
  const item = object(value, path);
  exactKeys(
    item,
    [
      "manage_repositories",
      "manage_secrets",
      "manage_runtimes",
      "delete_workspace",
    ],
    path,
  );
  return {
    manage_repositories: boolean(
      item.manage_repositories,
      `${path}.manage_repositories`,
    ),
    manage_secrets: boolean(item.manage_secrets, `${path}.manage_secrets`),
    manage_runtimes: boolean(item.manage_runtimes, `${path}.manage_runtimes`),
    delete_workspace: boolean(
      item.delete_workspace,
      `${path}.delete_workspace`,
    ),
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
  const schemaVersion = integer(
    response.schema_version,
    "workspace response.schema_version",
  );
  if (schemaVersion < 0) {
    throw new Error("workspace response.schema_version must be non-negative");
  }
  return {
    workspace_id: workspaceString(
      response.workspace_id,
      "workspace response.workspace_id",
    ),
    display_name: workspaceString(
      response.display_name,
      "workspace response.display_name",
    ),
    record_authority: workspaceString(
      response.record_authority,
      "workspace response.record_authority",
    ),
    schema_version: schemaVersion,
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
    workspace_id: repositoryString(
      response.workspace_id,
      "repository list response.workspace_id",
    ),
    items: repositoryArray(response.items, "repository list response.items")
      .map((
        item,
        index,
      ) => repositorySummary(item, `repository list response.items[${index}]`)),
    source: repositoryString(
      response.source,
      "repository list response.source",
    ),
    diagnostics: repositoryArray(
      response.diagnostics,
      "repository list response.diagnostics",
    ).map(
      (item, index) =>
        repositoryApiDiagnostic(
          item,
          `repository list response.diagnostics[${index}]`,
        ),
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
    workspace_id: repositoryString(
      response.workspace_id,
      "repository detail response.workspace_id",
    ),
    item: repositorySummary(response.item, "repository detail response.item"),
    source: repositoryString(
      response.source,
      "repository detail response.source",
    ),
  };
}

export function parseCreateWorkspaceRepositoryResponse(
  value: unknown,
): CreateWorkspaceRepositoryResponse {
  const response = object(value, "repository create response");
  exactKeys(
    response,
    ["replayed", "repository_key", "workspace_id"],
    "repository create response",
  );
  return {
    replayed: boolean(response.replayed, "repository create response.replayed"),
    repository_key: repositoryString(
      response.repository_key,
      "repository create response.repository_key",
    ),
    workspace_id: repositoryString(
      response.workspace_id,
      "repository create response.workspace_id",
    ),
  };
}

export function parseRepositoryApiError(value: unknown): RepositoryApiError {
  const response = object(value, "repository error response");
  exactKeys(
    response,
    ["diagnostics", "error", "message"],
    "repository error response",
  );
  return {
    diagnostics: response.diagnostics === undefined
      ? undefined
      : repositoryArray(
        response.diagnostics,
        "repository error response.diagnostics",
      ).map((item, index) =>
        repositoryApiDiagnostic(
          item,
          `repository error response.diagnostics[${index}]`,
        )
      ),
    error: repositoryString(response.error, "repository error response.error"),
    message: repositoryString(
      response.message,
      "repository error response.message",
    ),
  };
}

const SSH_CONNECTION_TRUST_STATES = new Set<RepositorySshConnectionTrustState>([
  "untrusted",
  "verified",
  "changed",
]);

function repositorySshHostKeyCandidate(
  value: unknown,
  path: string,
): RepositorySshHostKeyCandidate {
  const candidate = object(value, path);
  exactKeys(candidate, ["algorithm", "host_key", "fingerprint"], path);
  return {
    algorithm: string(candidate.algorithm, `${path}.algorithm`),
    host_key: string(candidate.host_key, `${path}.host_key`),
    fingerprint: string(candidate.fingerprint, `${path}.fingerprint`),
  };
}

export function parseRepositorySshConnectionProbeResponse(
  value: unknown,
): RepositorySshConnectionProbeResponse {
  const response = object(value, "repository SSH connection probe response");
  exactKeys(
    response,
    [
      "workspace_id",
      "repository_key",
      "runtime_id",
      "hostname",
      "port",
      "trust_state",
      "host_trust_id",
      "expected_host_trust_revision",
      "candidates",
    ],
    "repository SSH connection probe response",
  );
  const trustState = string(
    response.trust_state,
    "repository SSH connection probe response.trust_state",
  ) as RepositorySshConnectionTrustState;
  if (!SSH_CONNECTION_TRUST_STATES.has(trustState)) {
    throw new Error(
      "repository SSH connection probe response.trust_state is invalid",
    );
  }
  return {
    workspace_id: string(
      response.workspace_id,
      "repository SSH connection probe response.workspace_id",
    ),
    repository_key: string(
      response.repository_key,
      "repository SSH connection probe response.repository_key",
    ),
    runtime_id: string(
      response.runtime_id,
      "repository SSH connection probe response.runtime_id",
    ),
    hostname: string(
      response.hostname,
      "repository SSH connection probe response.hostname",
    ),
    port: integer(
      response.port,
      "repository SSH connection probe response.port",
    ),
    trust_state: trustState,
    host_trust_id: string(
      response.host_trust_id,
      "repository SSH connection probe response.host_trust_id",
    ),
    expected_host_trust_revision: response.expected_host_trust_revision === null
      ? null
      : integer(
        response.expected_host_trust_revision,
        "repository SSH connection probe response.expected_host_trust_revision",
      ),
    candidates: array(
      response.candidates,
      "repository SSH connection probe response.candidates",
    ).map(
      (candidate, index) =>
        repositorySshHostKeyCandidate(
          candidate,
          `repository SSH connection probe response.candidates[${index}]`,
        ),
    ),
  };
}

const WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES = 128;
const WORKSPACE_DELETION_MAX_REVISION_BYTES = 128;
const WORKSPACE_DELETION_MAX_BLOCKERS = 1024;
const WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS = 4096;
const WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES = 128;
const WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES = 512;

function deletionBoundedString(
  value: unknown,
  path: string,
  maxBytes: number,
): string {
  const candidate = string(value, path);
  if (new TextEncoder().encode(candidate).length > maxBytes) {
    throw new Error(`${path} is too long`);
  }
  return candidate;
}

function deletionBoundedArray(
  value: unknown,
  path: string,
  maxItems: number,
): unknown[] {
  const candidate = array(value, path);
  if (candidate.length > maxItems) {
    throw new Error(`${path} has too many items`);
  }
  return candidate;
}

const deletionStates = new Set<WorkspaceDeletionState>([
  "queued",
  "running",
  "blocked",
  "failed",
  "succeeded",
]);
const deletionBlockerKinds = new Set<WorkspaceDeletionBlockerKind>([
  "last_accessible_workspace",
  "revision_conflict",
  "dirty_workdir",
  "worker_removal_blocked",
  "workdir_removal_blocked",
  "retention_hold",
  "cleanup_unavailable",
]);

function deletionState(value: unknown, path: string): WorkspaceDeletionState {
  const candidate = string(value, path) as WorkspaceDeletionState;
  if (!deletionStates.has(candidate)) throw new Error(`${path} is invalid`);
  return candidate;
}

function deletionBlocker(
  value: unknown,
  path: string,
): WorkspaceDeletionBlocker {
  const item = object(value, path);
  exactKeys(item, ["kind", "resource_kind", "resource_key", "message"], path);
  const kind = string(
    item.kind,
    `${path}.kind`,
  ) as WorkspaceDeletionBlockerKind;
  if (!deletionBlockerKinds.has(kind)) {
    throw new Error(`${path}.kind is invalid`);
  }
  const resourceKind = optionalNullableString(
    item.resource_kind,
    `${path}.resource_kind`,
  );
  const resourceKey = optionalNullableString(
    item.resource_key,
    `${path}.resource_key`,
  );
  return {
    kind,
    resource_kind: resourceKind === undefined || resourceKind === null
      ? null
      : deletionBoundedString(
        resourceKind,
        `${path}.resource_kind`,
        WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES,
      ),
    resource_key: resourceKey === undefined || resourceKey === null
      ? null
      : deletionBoundedString(
        resourceKey,
        `${path}.resource_key`,
        WORKSPACE_DELETION_MAX_RESOURCE_VALUE_BYTES,
      ),
    message: deletionBoundedString(
      item.message,
      `${path}.message`,
      WORKSPACE_DELETION_MAX_BLOCKER_MESSAGE_BYTES,
    ),
  };
}

function deletionResourceCounts(
  value: unknown,
  path: string,
): WorkspaceDeletionResourceCounts {
  const item = object(value, path);
  exactKeys(item, [
    "workers",
    "workdirs",
    "repositories",
    "runtime_bindings",
    "secrets",
    "artifacts",
  ], path);
  return {
    workers: integer(item.workers, `${path}.workers`),
    workdirs: integer(item.workdirs, `${path}.workdirs`),
    repositories: integer(item.repositories, `${path}.repositories`),
    runtime_bindings: integer(
      item.runtime_bindings,
      `${path}.runtime_bindings`,
    ),
    secrets: integer(item.secrets, `${path}.secrets`),
    artifacts: integer(item.artifacts, `${path}.artifacts`),
  };
}

export function parseWorkspaceDeletionPreflightResponse(
  value: unknown,
): WorkspaceDeletionPreflightResponse {
  const item = object(value, "Workspace deletion preflight");
  exactKeys(item, [
    "workspace_id",
    "display_name",
    "expected_revision",
    "can_delete",
    "resources",
    "blockers",
  ], "Workspace deletion preflight");
  return {
    workspace_id: string(
      item.workspace_id,
      "Workspace deletion preflight.workspace_id",
    ),
    display_name: string(
      item.display_name,
      "Workspace deletion preflight.display_name",
    ),
    expected_revision: deletionBoundedString(
      item.expected_revision,
      "Workspace deletion preflight.expected_revision",
      WORKSPACE_DELETION_MAX_REVISION_BYTES,
    ),
    can_delete: boolean(
      item.can_delete,
      "Workspace deletion preflight.can_delete",
    ),
    resources: deletionResourceCounts(
      item.resources,
      "Workspace deletion preflight.resources",
    ),
    blockers: deletionBoundedArray(
      item.blockers,
      "Workspace deletion preflight.blockers",
      WORKSPACE_DELETION_MAX_BLOCKERS,
    ).map(
      (entry, index) =>
        deletionBlocker(
          entry,
          `Workspace deletion preflight.blockers[${index}]`,
        ),
    ),
  };
}

export function parseWorkspaceDeletionOperationResponse(
  value: unknown,
): WorkspaceDeletionOperationResponse {
  const item = object(value, "Workspace deletion operation");
  exactKeys(item, [
    "operation_id",
    "workspace_id",
    "display_name",
    "state",
    "resources",
    "child_operation_ids",
    "blockers",
    "failure_category",
    "created_at",
    "updated_at",
    "completed_at",
  ], "Workspace deletion operation");
  return {
    operation_id: deletionBoundedString(
      item.operation_id,
      "Workspace deletion operation.operation_id",
      WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES,
    ),
    workspace_id: string(
      item.workspace_id,
      "Workspace deletion operation.workspace_id",
    ),
    display_name: string(
      item.display_name,
      "Workspace deletion operation.display_name",
    ),
    state: deletionState(item.state, "Workspace deletion operation.state"),
    resources: deletionResourceCounts(
      item.resources,
      "Workspace deletion operation.resources",
    ),
    child_operation_ids: deletionBoundedArray(
      item.child_operation_ids,
      "Workspace deletion operation.child_operation_ids",
      WORKSPACE_DELETION_MAX_CHILD_OPERATION_IDS,
    ).map((entry, index) =>
      deletionBoundedString(
        entry,
        `Workspace deletion operation.child_operation_ids[${index}]`,
        WORKSPACE_DELETION_MAX_OPERATION_ID_BYTES,
      )
    ),
    blockers: deletionBoundedArray(
      item.blockers,
      "Workspace deletion operation.blockers",
      WORKSPACE_DELETION_MAX_BLOCKERS,
    ).map(
      (entry, index) =>
        deletionBlocker(
          entry,
          `Workspace deletion operation.blockers[${index}]`,
        ),
    ),
    failure_category: optionalNullableString(
      item.failure_category,
      "Workspace deletion operation.failure_category",
    ) ?? null,
    created_at: string(
      item.created_at,
      "Workspace deletion operation.created_at",
    ),
    updated_at: string(
      item.updated_at,
      "Workspace deletion operation.updated_at",
    ),
    completed_at: optionalNullableString(
      item.completed_at,
      "Workspace deletion operation.completed_at",
    ) ?? null,
  };
}

export function parseHostListResponse(value: unknown): HostListResponse {
  const response = object(value, "host list response");
  exactKeys(
    response,
    ["workspace_id", "limit", "items", "source", "diagnostics"],
    "host list response",
  );
  const limit = repositorySourceRevision(
    response.limit,
    "host list response.limit",
  );
  return {
    workspace_id: repositoryString(
      response.workspace_id,
      "host list response.workspace_id",
    ),
    limit,
    items: repositoryArray(response.items, "host list response.items").map(
      (item, index) => parseHostSummary(item, `host list response.items[${index}]`),
    ),
    source: repositoryString(response.source, "host list response.source"),
    diagnostics: repositoryArray(
      response.diagnostics,
      "host list response.diagnostics",
    ).map((item, index) =>
      repositoryApiDiagnostic(
        item,
        `host list response.diagnostics[${index}]`,
      )
    ),
  };
}

function parseHostSummary(value: unknown, path: string): HostSummary {
  const host = object(value, path);
  exactKeys(
    host,
    [
      "runtime_id",
      "host_id",
      "label",
      "kind",
      "status",
      "observed_at",
      "last_seen_at",
      "os",
      "arch",
      "diagnostics",
    ],
    path,
  );
  return {
    runtime_id: repositoryString(host.runtime_id, `${path}.runtime_id`),
    host_id: repositoryString(host.host_id, `${path}.host_id`),
    label: repositoryString(host.label, `${path}.label`),
    kind: repositoryString(host.kind, `${path}.kind`),
    status: repositoryString(host.status, `${path}.status`),
    observed_at: repositoryString(host.observed_at, `${path}.observed_at`),
    last_seen_at: optionalNullableRepositoryString(
      host.last_seen_at,
      `${path}.last_seen_at`,
    ),
    os: repositoryString(host.os, `${path}.os`),
    arch: repositoryString(host.arch, `${path}.arch`),
    diagnostics: repositoryArray(host.diagnostics, `${path}.diagnostics`).map(
      (item, index) =>
        repositoryApiDiagnostic(item, `${path}.diagnostics[${index}]`),
    ),
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
