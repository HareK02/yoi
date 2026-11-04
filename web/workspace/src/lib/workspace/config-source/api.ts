/** Rust契約に従うconfig treeの通信と応答検査。
 * tree digestはsource、projection digestは評価結果、contract fingerprintはtoolchain条件を指す。
 * ConfigProjectionValidatorはstatic template catalogの検査方針であり、鮮度tokenではない。
 * Browserの解析成功はServerのcommit受理を代替しない。
 */
import type {
  ConfigContentType,
  ConfigEntry,
  ConfigProjectionValidator,
  ConfigSchemaContribution,
  ConfigTreeSnapshot,
  ToolchainContract,
  WorkspaceConfigSchemaBundle,
  WorkspaceConfigTreeResponse,
} from "./types.ts";

import type {
  ConfigurationAdoptionReceipt,
  ConfigurationAdoptionRequest,
  ConfigurationPreviewResponse,
  ConfigurationReconciliation,
  ConfigurationSourceResponse,
} from "#lib/generated/configuration-adoption.generated.ts";

// 未使用のpolicy/catalogを認可判定へ転用しない。editorが使う観測値だけを型へ変換する。
export type ConfigurationEditingState = Pick<
  ConfigurationSourceResponse,
  "state" | "history_head" | "catalog_digest" | "expected_legacy_export" | "source_stored"
>;

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
// Schema bodies are documents, not short metadata. Keep them within the
// response budget; readJson also bounds the combined bundle and contributions.
const MAX_SCHEMA_SOURCE_BYTES = MAX_RESPONSE_BYTES;
const MAX_ERROR_BYTES = 4 * 1024;
const MAX_ENTRY_COUNT = 256;
const MAX_ENTRY_BYTES = 256 * 1024;
const MAX_PATH_BYTES = 512;
const MAX_STRING_BYTES = 4 * 1024;
const MAX_DIGEST_BYTES = 256;
const MAX_COLLECTION_ITEMS = 256;

export class ConfigSourceApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ConfigSourceApiError";
  }
}

type JsonRecord = Record<string, unknown>;

type Parser<T> = (value: unknown) => T;

function sourceTreeUrl(workspaceId: string): string {
  return `/api/w/${encodeURIComponent(workspaceId)}/config/source-tree`;
}

async function readJson<T>(
  response: Response,
  parser: Parser<T>,
): Promise<T> {
  const limit = response.ok ? MAX_RESPONSE_BYTES : MAX_ERROR_BYTES;
  const declared = response.headers.get("content-length");
  if (declared !== null) {
    const bytes = Number(declared);
    if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > limit) {
      throw new ConfigSourceApiError(
        response.ok
          ? "Workspace configuration response is too large."
          : "Workspace configuration request failed.",
        response.ok ? 502 : response.status,
      );
    }
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > limit) {
    throw new ConfigSourceApiError(
      response.ok
        ? "Workspace configuration response is too large."
        : "Workspace configuration request failed.",
      response.ok ? 502 : response.status,
    );
  }
  const text = new TextDecoder().decode(bytes);
  if (!response.ok) {
    throw new ConfigSourceApiError(
      text || `${response.status} ${response.statusText}`,
      response.status,
    );
  }
  let value: unknown;
  try {
    value = JSON.parse(text) as unknown;
  } catch {
    throw schemaError();
  }
  return parser(value);
}

function schemaError(): ConfigSourceApiError {
  return new ConfigSourceApiError(
    "Workspace configuration returned an invalid response.",
    502,
  );
}

function objectValue(value: unknown): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw schemaError();
  }
  return value as JsonRecord;
}

function record(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = [],
): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw schemaError();
  }
  const result = value as JsonRecord;
  const allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !(key in result)) ||
    Object.keys(result).some((key) => !allowed.has(key))
  ) {
    throw schemaError();
  }
  return result;
}

function boundedString(value: unknown, maxBytes = MAX_STRING_BYTES): string {
  if (
    typeof value !== "string" ||
    new TextEncoder().encode(value).byteLength > maxBytes
  ) {
    throw schemaError();
  }
  return value;
}

function safeInteger(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw schemaError();
  }
  return value as number;
}

function boundedArray(value: unknown): unknown[] {
  if (!Array.isArray(value) || value.length > MAX_COLLECTION_ITEMS) {
    throw schemaError();
  }
  return value;
}

function parseContentType(value: unknown): ConfigContentType {
  if (value !== "decodal" && value !== "text") throw schemaError();
  return value;
}

export function parseConfigEntry(value: unknown): ConfigEntry {
  const item = record(value, [
    "path",
    "content_type",
    "content",
    "content_digest",
  ]);
  return {
    path: boundedString(item.path, MAX_PATH_BYTES),
    content_type: parseContentType(item.content_type),
    content: boundedString(item.content, MAX_ENTRY_BYTES),
    content_digest: boundedString(item.content_digest, MAX_DIGEST_BYTES),
  };
}

function parseStringMap(value: unknown): Record<string, string> {
  const item = objectValue(value);
  const entries = Object.entries(item);
  if (entries.length > MAX_COLLECTION_ITEMS) throw schemaError();
  const result: Record<string, string> = {};
  for (const [key, entry] of entries) {
    const boundedKey = boundedString(key, MAX_PATH_BYTES);
    result[boundedKey] = boundedString(entry, MAX_PATH_BYTES);
  }
  return result;
}

function parseProjectionValidator(value: unknown): ConfigProjectionValidator {
  const item = record(value, ["kind", "namespace"], ["key_aliases"]);
  if (item.kind !== "static_template_catalog") throw schemaError();
  return {
    kind: "static_template_catalog",
    namespace: boundedString(item.namespace, MAX_PATH_BYTES),
    key_aliases: item.key_aliases === undefined
      ? {}
      : parseStringMap(item.key_aliases),
  };
}

function parseContribution(value: unknown): ConfigSchemaContribution {
  const item = record(
    value,
    ["provider_id", "namespace", "version", "source", "source_digest"],
    ["projection_validator", "authoring_source"],
  );
  const projection = item.projection_validator === undefined
    ? undefined
    : parseProjectionValidator(item.projection_validator);
  return {
    provider_id: boundedString(item.provider_id),
    namespace: boundedString(item.namespace),
    version: boundedString(item.version),
    source: boundedString(item.source, MAX_SCHEMA_SOURCE_BYTES),
    ...(item.authoring_source === undefined ? {} : {
      authoring_source: boundedString(
        item.authoring_source,
        MAX_SCHEMA_SOURCE_BYTES,
      ),
    }),
    ...(projection === undefined ? {} : { projection_validator: projection }),
    source_digest: boundedString(item.source_digest, MAX_DIGEST_BYTES),
  };
}

function parseSchemaBundle(value: unknown): WorkspaceConfigSchemaBundle {
  const item = record(value, ["contributions", "source", "fingerprint"]);
  return {
    contributions: boundedArray(item.contributions).map(parseContribution),
    source: boundedString(item.source, MAX_SCHEMA_SOURCE_BYTES),
    fingerprint: boundedString(item.fingerprint, MAX_DIGEST_BYTES),
  };
}

function parseToolchainContract(value: unknown): ToolchainContract {
  const item = record(value, [
    "contract_version",
    "decodal_version",
    "schema_version",
    "entrypoints",
    "import_policy_version",
    "schema_bundle",
    "fingerprint",
  ]);
  return {
    contract_version: safeInteger(item.contract_version),
    decodal_version: boundedString(item.decodal_version),
    schema_version: safeInteger(item.schema_version),
    entrypoints: boundedArray(item.entrypoints).map((entry) =>
      boundedString(entry, MAX_PATH_BYTES)
    ),
    import_policy_version: safeInteger(item.import_policy_version),
    schema_bundle: parseSchemaBundle(item.schema_bundle),
    fingerprint: boundedString(item.fingerprint, MAX_DIGEST_BYTES),
  };
}

export function parseConfigTreeSnapshot(value: unknown): ConfigTreeSnapshot {
  const item = record(value, ["digest", "entries"]);
  const entriesRecord = objectValue(item.entries);
  const entries = Object.entries(entriesRecord);
  if (entries.length > MAX_ENTRY_COUNT) throw schemaError();
  const parsedEntries: Record<string, ConfigEntry> = {};
  for (const [path, entry] of entries) {
    const boundedPath = boundedString(path, MAX_PATH_BYTES);
    const parsed = parseConfigEntry(entry);
    if (parsed.path !== boundedPath) throw schemaError();
    parsedEntries[boundedPath] = parsed;
  }
  return {
    digest: boundedString(item.digest, MAX_DIGEST_BYTES),
    entries: parsedEntries,
  };
}

export function parseWorkspaceConfigTreeResponse(
  value: unknown,
): WorkspaceConfigTreeResponse {
  const item = record(value, ["snapshot", "contract", "projection_digest"]);
  return {
    snapshot: parseConfigTreeSnapshot(item.snapshot),
    contract: parseToolchainContract(item.contract),
    projection_digest: boundedString(
      item.projection_digest,
      MAX_DIGEST_BYTES,
    ),
  };
}

function adoptionUrl(workspaceId: string): string {
  return `/api/w/${encodeURIComponent(workspaceId)}/config/adoption`;
}

function nullableDigest(value: unknown): string | null {
  return value === null ? null : boundedString(value, MAX_DIGEST_BYTES);
}

// 非表示のpolicy/catalogも大きさと深さを制限するが、その内部を型付け・再解釈しない。
function checkOpaqueData(value: unknown, depth = 0, budget = { nodes: 65536 }): void {
  if (depth > 16 || --budget.nodes < 0) throw schemaError();
  if (value === null || typeof value === "boolean") return;
  if (typeof value === "string") {
    boundedString(value);
    return;
  }
  if (typeof value === "number" && Number.isFinite(value)) return;
  if (Array.isArray(value)) {
    for (const child of boundedArray(value)) checkOpaqueData(child, depth + 1, budget);
    return;
  }
  const entries = Object.entries(objectValue(value));
  if (entries.length > MAX_COLLECTION_ITEMS) throw schemaError();
  for (const [key, child] of entries) {
    boundedString(key);
    checkOpaqueData(child, depth + 1, budget);
  }
}

export function parseConfigurationSourceResponse(value: unknown): ConfigurationEditingState {
  const item = record(value, [
    "state", "history_head", "source_stored", "catalog", "catalog_digest",
    "expected_legacy_export", "policies", "migration_diagnostics",
  ]);
  if (typeof item.source_stored !== "boolean") throw schemaError();
  objectValue(item.catalog);
  boundedArray(item.policies);
  boundedArray(item.migration_diagnostics);
  checkOpaqueData([item.catalog, item.policies, item.migration_diagnostics]);
  return {
    state: parseWorkspaceConfigTreeResponse(item.state),
    history_head: nullableDigest(item.history_head),
    source_stored: item.source_stored,
    catalog_digest: boundedString(item.catalog_digest, MAX_DIGEST_BYTES),
    expected_legacy_export: nullableDigest(item.expected_legacy_export),
  };
}

export function parseConfigurationPreviewResponse(value: unknown): ConfigurationPreviewResponse {
  const item = record(value, ["expected_head", "state", "changed_paths", "changed_subjects"]);
  return {
    expected_head: nullableDigest(item.expected_head),
    state: parseWorkspaceConfigTreeResponse(item.state),
    changed_paths: boundedArray(item.changed_paths).map((path) => boundedString(path, MAX_PATH_BYTES)),
    changed_subjects: boundedArray(item.changed_subjects).map((subject) => boundedString(subject)),
  };
}

export function parseConfigurationAdoptionReceipt(value: unknown): ConfigurationAdoptionReceipt {
  const item = record(value, [
    "workspace_id", "operation_id", "history_head", "state", "source_stored",
    "policy_committed", "runtime_application",
  ]);
  if (item.source_stored !== true || item.policy_committed !== true ||
    item.runtime_application !== "next_current_policy_check") throw schemaError();
  return {
    workspace_id: boundedString(item.workspace_id),
    operation_id: boundedString(item.operation_id),
    history_head: boundedString(item.history_head, MAX_DIGEST_BYTES),
    state: parseWorkspaceConfigTreeResponse(item.state),
    source_stored: true,
    policy_committed: true,
    runtime_application: "next_current_policy_check",
  };
}

export function parseConfigurationReconciliation(value: unknown): ConfigurationReconciliation {
  const status = objectValue(value).status;
  if (status === "adopted") {
    const item = record(value, ["status", "receipt"]);
    return { status, receipt: parseConfigurationAdoptionReceipt(item.receipt) };
  }
  if (status === "pending_transfer") {
    const item = record(value, ["status", "transfer_identity", "export_digest"]);
    return {
      status,
      transfer_identity: boundedString(item.transfer_identity),
      export_digest: boundedString(item.export_digest, MAX_DIGEST_BYTES),
    };
  }
  if (status === "not_observed") {
    record(value, ["status"]);
    return { status };
  }
  throw schemaError();
}

function matchingReceipt(
  receipt: ConfigurationAdoptionReceipt,
  workspaceId: string,
  request: ConfigurationAdoptionRequest,
): ConfigurationAdoptionReceipt {
  if (receipt.workspace_id !== workspaceId || receipt.operation_id !== request.operation_id) throw schemaError();
  return receipt;
}

async function postAdoption<T>(
  workspaceId: string,
  action: "preview" | "commit" | "reconcile",
  request: ConfigurationAdoptionRequest,
  parser: Parser<T>,
  fetcher: typeof fetch,
): Promise<T> {
  return await readJson(
    await fetcher(`${adoptionUrl(workspaceId)}/${action}`, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify(request),
    }),
    parser,
  );
}

export async function fetchConfigTree(
  workspaceId: string,
  fetcher: typeof fetch = fetch,
): Promise<ConfigurationEditingState> {
  return await readJson(
    await fetcher(adoptionUrl(workspaceId), {
      headers: { accept: "application/json" },
    }),
    parseConfigurationSourceResponse,
  );
}

export async function fetchConfigEntry(
  workspaceId: string,
  path: string,
  fetcher: typeof fetch = fetch,
): Promise<ConfigEntry> {
  return await readJson(
    await fetcher(
      `${sourceTreeUrl(workspaceId)}/entries/${encodeURIComponent(path)}`,
      { headers: { accept: "application/json" } },
    ),
    parseConfigEntry,
  );
}

export async function fetchConfigHistory(
  workspaceId: string,
  contentDigest: string,
  fetcher: typeof fetch = fetch,
): Promise<ConfigTreeSnapshot> {
  return await readJson(
    await fetcher(`${sourceTreeUrl(workspaceId)}/history/${encodeURIComponent(contentDigest)}`, {
      headers: { accept: "application/json" },
    }),
    parseConfigTreeSnapshot,
  );
}

export async function previewConfigTree(
  workspaceId: string,
  request: ConfigurationAdoptionRequest,
  fetcher: typeof fetch = fetch,
): Promise<ConfigurationPreviewResponse> {
  const preview = await postAdoption(workspaceId, "preview", request, parseConfigurationPreviewResponse, fetcher);
  if (preview.expected_head !== request.expected_head) throw schemaError();
  return preview;
}

export async function commitConfigTree(
  workspaceId: string,
  request: ConfigurationAdoptionRequest,
  fetcher: typeof fetch = fetch,
): Promise<ConfigurationAdoptionReceipt> {
  return matchingReceipt(
    await postAdoption(workspaceId, "commit", request, parseConfigurationAdoptionReceipt, fetcher),
    workspaceId,
    request,
  );
}

export async function reconcileConfigTree(
  workspaceId: string,
  request: ConfigurationAdoptionRequest,
  fetcher: typeof fetch = fetch,
): Promise<ConfigurationReconciliation> {
  const result = await postAdoption(workspaceId, "reconcile", request, parseConfigurationReconciliation, fetcher);
  if (result.status === "adopted") matchingReceipt(result.receipt, workspaceId, request);
  return result;
}
