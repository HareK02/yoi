import type {
  ConfigCommitRequest,
  ConfigContentType,
  ConfigEntry,
  ConfigProjectionValidator,
  ConfigSchemaContribution,
  ConfigTreeSnapshot,
  ToolchainContract,
  WorkspaceConfigSchemaBundle,
  WorkspaceConfigTreeResponse,
} from "./types.ts";

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;
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
    ["projection_validator"],
  );
  const projection = item.projection_validator === undefined
    ? undefined
    : parseProjectionValidator(item.projection_validator);
  return {
    provider_id: boundedString(item.provider_id),
    namespace: boundedString(item.namespace),
    version: boundedString(item.version),
    source: boundedString(item.source),
    ...(projection === undefined ? {} : { projection_validator: projection }),
    source_digest: boundedString(item.source_digest, MAX_DIGEST_BYTES),
  };
}

function parseSchemaBundle(value: unknown): WorkspaceConfigSchemaBundle {
  const item = record(value, ["contributions", "source", "fingerprint"]);
  return {
    contributions: boundedArray(item.contributions).map(parseContribution),
    source: boundedString(item.source),
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
  const item = record(value, ["revision", "digest", "entries"]);
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
    revision: safeInteger(item.revision),
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

export async function fetchConfigTree(
  workspaceId: string,
  fetcher: typeof fetch = fetch,
): Promise<WorkspaceConfigTreeResponse> {
  return await readJson(
    await fetcher(sourceTreeUrl(workspaceId), {
      headers: { accept: "application/json" },
    }),
    parseWorkspaceConfigTreeResponse,
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

export async function fetchConfigRevision(
  workspaceId: string,
  revision: number,
  fetcher: typeof fetch = fetch,
): Promise<ConfigTreeSnapshot> {
  return await readJson(
    await fetcher(`${sourceTreeUrl(workspaceId)}/revisions/${revision}`, {
      headers: { accept: "application/json" },
    }),
    parseConfigTreeSnapshot,
  );
}

export async function commitConfigTree(
  workspaceId: string,
  request: ConfigCommitRequest,
  fetcher: typeof fetch = fetch,
): Promise<WorkspaceConfigTreeResponse> {
  return await readJson(
    await fetcher(`${sourceTreeUrl(workspaceId)}/commit`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    }),
    parseWorkspaceConfigTreeResponse,
  );
}
