import type {
  RepositoryAccessProjection,
  RepositorySshCredential,
  RepositorySshHostTrust,
} from "../../generated/repository-access-api.ts";

export class RepositoryAccessSchemaError extends Error {
  constructor(path: string, expected: string) {
    super(
      `Repository Access response schema mismatch at ${path}: expected ${expected}`,
    );
    this.name = "RepositoryAccessSchemaError";
  }
}

export function parseRepositorySshCredentials(
  value: unknown,
): RepositorySshCredential[] {
  return readArray(value, "credentials").map((entry, index) =>
    parseRepositorySshCredential(entry, `credentials[${index}]`)
  );
}

export function parseRepositorySshCredential(
  value: unknown,
  path = "credential",
): RepositorySshCredential {
  const record = readRecord(value, path, [
    "credential_id",
    "workspace_id",
    "name",
    "public_key_algorithm",
    "public_key_fingerprint",
    "current_revision",
    "status",
    "created_at",
    "rotated_at",
    "referenced_repositories",
  ]);
  readString(record, "credential_id", path);
  readString(record, "workspace_id", path);
  readString(record, "name", path);
  readString(record, "public_key_algorithm", path);
  readString(record, "public_key_fingerprint", path);
  readRevision(record, "current_revision", path);
  readString(record, "status", path);
  readString(record, "created_at", path);
  readNullableString(record, "rotated_at", path);
  readStringArray(record, "referenced_repositories", path);
  return record as RepositorySshCredential;
}

export function parseRepositorySshHostTrusts(
  value: unknown,
): RepositorySshHostTrust[] {
  return readArray(value, "host_trusts").map((entry, index) =>
    parseRepositorySshHostTrust(entry, `host_trusts[${index}]`)
  );
}

export function parseRepositorySshHostTrust(
  value: unknown,
  path = "host_trust",
): RepositorySshHostTrust {
  const record = readRecord(value, path, [
    "host_trust_id",
    "workspace_id",
    "hostname",
    "port",
    "key_algorithm",
    "host_key",
    "fingerprint",
    "current_revision",
    "created_at",
    "updated_at",
    "referenced_repositories",
  ]);
  readString(record, "host_trust_id", path);
  readString(record, "workspace_id", path);
  readString(record, "hostname", path);
  const port = readInteger(record, "port", path);
  if (port < 1 || port > 65_535) {
    throw new RepositoryAccessSchemaError(
      `${path}.port`,
      "an integer from 1 to 65535",
    );
  }
  readString(record, "key_algorithm", path);
  readString(record, "host_key", path);
  readString(record, "fingerprint", path);
  readRevision(record, "current_revision", path);
  readString(record, "created_at", path);
  readString(record, "updated_at", path);
  readStringArray(record, "referenced_repositories", path);
  return record as RepositorySshHostTrust;
}

export function parseRepositoryAccessProjection(
  value: unknown,
): RepositoryAccessProjection {
  const path = "access_projection";
  const record = readRecord(value, path, [
    "workspace_id",
    "config_revision",
    "projection_digest",
    "bindings",
  ]);
  readString(record, "workspace_id", path);
  readRevision(record, "config_revision", path);
  readString(record, "projection_digest", path);
  const bindings = readArray(record.bindings, `${path}.bindings`);
  bindings.forEach((binding, index) => {
    const bindingPath = `${path}.bindings[${index}]`;
    const bindingRecord = readRecord(binding, bindingPath, [
      "repository_key",
      "credential_id",
      "host_trust_id",
      "access",
    ]);
    readString(bindingRecord, "repository_key", bindingPath);
    readString(bindingRecord, "credential_id", bindingPath);
    readString(bindingRecord, "host_trust_id", bindingPath);
    const access = readString(bindingRecord, "access", bindingPath);
    if (access !== "read_only" && access !== "read_write") {
      throw new RepositoryAccessSchemaError(
        `${bindingPath}.access`,
        '"read_only" or "read_write"',
      );
    }
  });
  return record as RepositoryAccessProjection;
}

function readRecord(
  value: unknown,
  path: string,
  allowedKeys: readonly string[],
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new RepositoryAccessSchemaError(path, "an object");
  }
  const record = value as Record<string, unknown>;
  const unknownKey = Object.keys(record).find((key) =>
    !allowedKeys.includes(key)
  );
  if (unknownKey !== undefined) {
    throw new RepositoryAccessSchemaError(
      `${path}.${unknownKey}`,
      "no unknown field",
    );
  }
  return record;
}

function readArray(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new RepositoryAccessSchemaError(path, "an array");
  }
  return value;
}

function readString(
  record: Record<string, unknown>,
  key: string,
  path: string,
): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new RepositoryAccessSchemaError(`${path}.${key}`, "a string");
  }
  return value;
}

function readNullableString(
  record: Record<string, unknown>,
  key: string,
  path: string,
): string | null {
  const value = record[key];
  if (value !== null && typeof value !== "string") {
    throw new RepositoryAccessSchemaError(`${path}.${key}`, "a string or null");
  }
  return value;
}

function readStringArray(
  record: Record<string, unknown>,
  key: string,
  path: string,
): string[] {
  const values = readArray(record[key], `${path}.${key}`);
  values.forEach((value, index) => {
    if (typeof value !== "string") {
      throw new RepositoryAccessSchemaError(
        `${path}.${key}[${index}]`,
        "a string",
      );
    }
  });
  return values as string[];
}

function readInteger(
  record: Record<string, unknown>,
  key: string,
  path: string,
): number {
  const value = record[key];
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new RepositoryAccessSchemaError(`${path}.${key}`, "a safe integer");
  }
  return value;
}

function readRevision(
  record: Record<string, unknown>,
  key: string,
  path: string,
): number {
  const revision = readInteger(record, key, path);
  if (revision < 0) {
    throw new RepositoryAccessSchemaError(
      `${path}.${key}`,
      "a non-negative safe integer",
    );
  }
  return revision;
}
