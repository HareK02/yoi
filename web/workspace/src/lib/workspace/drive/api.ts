import type {
  DriveApiError,
  DriveEntry,
  DriveEntryRef,
  DriveListResponse,
  DriveMutation,
  DriveMutationResponse,
  DriveReadTextResponse,
  DriveRequestStatusResponse,
} from "#lib/generated/drive-api.ts";
import { workspaceApiPath } from "#lib/workspace/api/http.ts";

export type {
  DriveEntry,
  DriveEntryRef,
  DriveMutation,
} from "#lib/generated/drive-api.ts";
export const DRIVE_RESPONSE_MAX_BYTES = 256 * 1024;
// A 64 KiB UTF-8 text can expand to six times its length in JSON escapes.
// Keep the JSON wire bound separate from the raster preview byte limit.
export const DRIVE_JSON_MAX_BYTES = 512 * 1024;
export const DRIVE_TEXT_MAX_BYTES = 64 * 1024;
export const DRIVE_FILE_MAX_BYTES = 16 * 1024 * 1024;
const MAX_DECIMAL = "9223372036854775807";

export class DriveContractError extends Error {
  constructor() {
    super("Invalid Drive payload");
  }
}
export class DriveRequestError extends Error {
  constructor(
    public readonly code: DriveApiError["code"],
    public readonly classification: DriveApiError["classification"],
  ) {
    super(
      classification === "unknown"
        ? "Drive outcome unknown; check request status"
        : `Drive request failed: ${code}`,
    );
  }
}
function invalid(): never {
  throw new DriveContractError();
}
function record(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    invalid();
  }
  return value as Record<string, unknown>;
}
function string(value: unknown, maximum = 4096): string {
  if (typeof value !== "string" || value.length > maximum) invalid();
  return value;
}
function boolean(value: unknown): boolean {
  if (typeof value !== "boolean") invalid();
  return value;
}
function nullableString(value: unknown): string | null {
  return value === null ? null : string(value);
}
export function parseDriveDecimal(value: unknown): string {
  const result = string(value, 19);
  if (
    !/^[1-9][0-9]*$/.test(result) ||
    (result.length === 19 && result > MAX_DECIMAL)
  ) invalid();
  return result;
}
function hasControl(value: string, includeSpace = false): boolean {
  return Array.from(value).some((char) =>
    char.charCodeAt(0) <= (includeSpace ? 32 : 31)
  );
}
function workspace(value: string): string {
  if (
    !value || value === "." || value === ".." || hasControl(value, true) ||
    /[/\\?#]/.test(value)
  ) invalid();
  return string(value, 256);
}
export function parseDriveEntryRef(
  value: unknown,
  workspaceId: string,
): DriveEntryRef {
  const v = record(value);
  if (string(v.workspace_id) !== workspace(workspaceId)) invalid();
  return { workspace_id: workspaceId, node_id: parseDriveDecimal(v.node_id) };
}
function base(workspaceId: string): string {
  return workspaceApiPath(workspace(workspaceId), "/drive");
}
function query(
  values: Record<string, string | number | boolean | null | undefined>,
): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value !== null && value !== undefined) params.set(key, String(value));
  }
  return `?${params}`;
}
function refQuery(
  ref: DriveEntryRef,
  workspaceId: string,
): { entry_workspace_id: string; id: string } {
  const parsed = parseDriveEntryRef(ref, workspaceId);
  return { entry_workspace_id: parsed.workspace_id, id: parsed.node_id };
}
/** Only service latest links for this exact entry, without credentials or extra query keys. */
export function parseDriveLatestUrl(
  value: unknown,
  entry: DriveEntryRef,
  kind: DriveEntry["kind"],
): string {
  const path = string(value);
  const endpoint = `${base(entry.workspace_id)}/${
    kind === "file" ? "download" : "metadata"
  }`;
  if (
    !path.startsWith(`${endpoint}?`) || hasControl(path, true) ||
    /[\\#]/.test(path)
  ) {
    invalid();
  }
  const params = new URLSearchParams(path.slice(endpoint.length + 1));
  if (
    [...params].length !== 2 ||
    params.get("entry_workspace_id") !== entry.workspace_id ||
    params.get("id") !== entry.node_id
  ) invalid();
  return path;
}
function mime(value: unknown): string {
  const result = string(value, 128);
  if (!/^[a-zA-Z0-9!#$&^_.+-]+\/[a-zA-Z0-9!#$&^_.+-]+$/.test(result)) invalid();
  return result.toLowerCase();
}
export function parseDriveEntry(
  value: unknown,
  workspaceId: string,
): DriveEntry {
  const v = record(value);
  const entry = parseDriveEntryRef(v.entry, workspaceId);
  const parent = v.parent === null
    ? null
    : parseDriveEntryRef(v.parent, workspaceId);
  if (v.kind !== "folder" && v.kind !== "file") invalid();
  const size = v.size;
  if (v.kind === "file") {
    if (
      typeof size !== "number" || !Number.isSafeInteger(size) || size < 0 ||
      size > DRIVE_FILE_MAX_BYTES || parent === null
    ) invalid();
  } else if (size !== null || v.content_type !== null) invalid();
  return {
    entry,
    parent,
    kind: v.kind,
    name: string(v.name, 255),
    revision: parseDriveDecimal(v.revision),
    size: size as number | null,
    content_type: v.content_type === null ? null : mime(v.content_type),
    updated_by: string(v.updated_by),
    updated_at: string(v.updated_at, 128),
    latest_url: parseDriveLatestUrl(v.latest_url, entry, v.kind),
  };
}
export function parseDriveListResponse(
  value: unknown,
  workspaceId: string,
  maximum = 200,
): DriveListResponse {
  const v = record(value);
  if (!Array.isArray(v.entries) || v.entries.length > maximum) invalid();
  const entries = v.entries.map((entry) => parseDriveEntry(entry, workspaceId));
  if (
    new Set(entries.map((entry) => entry.entry.node_id)).size !== entries.length
  ) invalid();
  return { entries, next_after: nullableString(v.next_after) };
}
export function parseDriveReadTextResponse(
  value: unknown,
  workspaceId: string,
): DriveReadTextResponse {
  const v = record(value);
  const entry = parseDriveEntry(v.entry, workspaceId);
  const text = string(v.text, DRIVE_TEXT_MAX_BYTES);
  if (
    entry.kind !== "file" ||
    new TextEncoder().encode(text).length > DRIVE_TEXT_MAX_BYTES
  ) invalid();
  return { entry, text, truncated: boolean(v.truncated) };
}
export function parseDriveMutationResponse(
  value: unknown,
  workspaceId: string,
  requestId: string,
): DriveMutationResponse {
  const v = record(value);
  if (v.request_id !== requestId) invalid();
  return {
    request_id: requestId,
    entry: v.entry === null ? null : parseDriveEntry(v.entry, workspaceId),
  };
}
export function parseDriveRequestStatusResponse(
  value: unknown,
  workspaceId: string,
  requestId: string,
): DriveRequestStatusResponse {
  const v = record(value);
  if (
    v.request_id !== requestId ||
    (v.state !== "committed" && v.state !== "uncommitted")
  ) invalid();
  if ((v.state === "uncommitted") !== (v.response === null)) invalid();
  return {
    request_id: requestId,
    state: v.state,
    response: v.response === null
      ? null
      : parseDriveMutationResponse(v.response, workspaceId, requestId),
  };
}
export function parseDriveApiError(value: unknown): DriveApiError {
  const v = record(value);
  const codes = [
    "denied",
    "not_found",
    "conflict",
    "invalid",
    "limit",
    "storage_unavailable",
    "outcome_unknown",
  ];
  if (
    !codes.includes(v.code as string) ||
    (v.classification !== "not_committed" && v.classification !== "unknown") ||
    (v.code === "outcome_unknown") !== (v.classification === "unknown")
  ) invalid();
  return {
    code: v.code as DriveApiError["code"],
    classification: v.classification,
    message: string(v.message),
  };
}
export async function readBoundedDriveBytes(
  response: Response,
  maximum = DRIVE_RESPONSE_MAX_BYTES,
): Promise<Uint8Array<ArrayBuffer>> {
  const length = response.headers.get("content-length");
  if (
    length !== null &&
    (!/^[0-9]+$/.test(length) || BigInt(length) > BigInt(maximum))
  ) {
    await response.body?.cancel();
    invalid();
  }
  const reader = response.body?.getReader();
  if (!reader) invalid();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maximum) {
        await reader.cancel();
        invalid();
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
}
export type DriveUploadTarget =
  | {
    operation: "create";
    parent: DriveEntryRef;
    name: string;
    content_type: string;
  }
  | {
    operation: "update";
    id: DriveEntryRef;
    expected_revision: string;
    content_type: string;
  };
export type DrivePageOptions = { after?: string | null; limit?: number };
export interface DriveClient {
  root(workspaceId: string, signal?: AbortSignal): Promise<DriveEntry>;
  metadata(
    workspaceId: string,
    ref: DriveEntryRef,
    signal?: AbortSignal,
  ): Promise<DriveEntry>;
  list(
    workspaceId: string,
    ref: DriveEntryRef,
    options?: DrivePageOptions,
    signal?: AbortSignal,
  ): Promise<DriveListResponse>;
  search(
    workspaceId: string,
    text: string,
    includeText?: boolean,
    options?: DrivePageOptions,
    signal?: AbortSignal,
  ): Promise<DriveListResponse>;
  readText(
    workspaceId: string,
    ref: DriveEntryRef,
    signal?: AbortSignal,
  ): Promise<DriveReadTextResponse>;
  image(
    workspaceId: string,
    entry: DriveEntry,
    signal?: AbortSignal,
  ): Promise<Blob>;
  mutate(
    workspaceId: string,
    requestId: string,
    mutation: DriveMutation,
    signal?: AbortSignal,
  ): Promise<DriveMutationResponse>;
  upload(
    workspaceId: string,
    requestId: string,
    blob: Blob,
    target: DriveUploadTarget,
    signal?: AbortSignal,
  ): Promise<DriveMutationResponse>;
  status(
    workspaceId: string,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<DriveRequestStatusResponse>;
}
export function driveDownloadUrl(
  workspaceId: string,
  ref: DriveEntryRef,
  revision?: string,
): string {
  return `${base(workspaceId)}/download${
    query({
      ...refQuery(ref, workspaceId),
      expected_revision: revision === undefined
        ? null
        : parseDriveDecimal(revision),
    })
  }`;
}
function requestId(value: string): string {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(value)) invalid();
  return value;
}
function name(value: string): string {
  if (
    !value || value === "." || value === ".." || hasControl(value) ||
    /[/\\]/.test(value) ||
    new TextEncoder().encode(value).length > 255
  ) invalid();
  return value;
}
function validateMutation(value: DriveMutation, workspaceId: string): void {
  if ("id" in value) {
    parseDriveEntryRef(value.id, workspaceId);
    parseDriveDecimal(value.expected_revision);
  }
  if ("parent" in value) parseDriveEntryRef(value.parent, workspaceId);
  if ("name" in value) name(value.name);
  if ("text" in value) {
    if (new TextEncoder().encode(value.text).length > DRIVE_TEXT_MAX_BYTES) {
      throw new DriveRequestError("limit", "not_committed");
    }
    mime(value.content_type);
  }
}
function page(options: DrivePageOptions, maximum: number): DrivePageOptions {
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 ||
      options.limit > maximum)
  ) invalid();
  if (options.after !== undefined && options.after !== null) {
    string(options.after);
  }
  return options;
}
export function createDriveClient(fetchFn: typeof fetch = fetch): DriveClient {
  async function response(
    path: string,
    init: RequestInit = {},
    mutation = false,
  ): Promise<Response> {
    // Fetch invocation is the conservative transmission boundary. An abort/network
    // error after this point cannot prove that the server did not commit.
    try {
      return await fetchFn(path, {
        ...init,
        credentials: "same-origin",
        cache: "no-store",
        redirect: "error",
      });
    } catch {
      throw new DriveRequestError(
        mutation ? "outcome_unknown" : "storage_unavailable",
        mutation ? "unknown" : "not_committed",
      );
    }
  }
  async function checked(res: Response): Promise<void> {
    if (res.ok) return;
    const payload: unknown = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(
        await readBoundedDriveBytes(res, DRIVE_JSON_MAX_BYTES),
      ),
    );
    const error = parseDriveApiError(payload);
    throw new DriveRequestError(error.code, error.classification);
  }
  async function json<T>(
    path: string,
    parse: (value: unknown) => T,
    init?: RequestInit,
    mutation = false,
  ): Promise<T> {
    const res = await response(path, init, mutation);
    try {
      await checked(res);
      return parse(
        JSON.parse(
          new TextDecoder("utf-8", { fatal: true }).decode(
            await readBoundedDriveBytes(res, DRIVE_JSON_MAX_BYTES),
          ),
        ),
      );
    } catch (error) {
      if (error instanceof DriveRequestError) throw error;
      if (mutation) throw new DriveRequestError("outcome_unknown", "unknown");
      throw error;
    }
  }
  function exact(entry: DriveEntry, ref: DriveEntryRef): DriveEntry {
    if (entry.entry.node_id !== ref.node_id) invalid();
    return entry;
  }
  return {
    root: (ws, signal) =>
      json(`${base(ws)}/root`, (v) => {
        const entry = parseDriveEntry(v, ws);
        if (entry.kind !== "folder" || entry.parent !== null) invalid();
        return entry;
      }, { signal }),
    metadata: (ws, ref, signal) =>
      json(
        `${base(ws)}/metadata${query(refQuery(ref, ws))}`,
        (v) => exact(parseDriveEntry(v, ws), ref),
        { signal },
      ),
    list: (ws, ref, options = {}, signal) =>
      json(
        `${base(ws)}/list${
          query({ ...refQuery(ref, ws), ...page(options, 200) })
        }`,
        (v) => {
          const result = parseDriveListResponse(v, ws);
          if (
            result.entries.some((e) => e.parent?.node_id !== ref.node_id)
          ) invalid();
          return result;
        },
        { signal },
      ),
    search: (ws, text, includeText = false, options = {}, signal) =>
      json(
        `${base(ws)}/search${
          query({
            query: string(text),
            include_text: includeText,
            ...page(options, 128),
          })
        }`,
        (v) => parseDriveListResponse(v, ws, 128),
        { signal },
      ),
    readText: (ws, ref, signal) =>
      json(
        `${base(ws)}/read-text${
          query({ ...refQuery(ref, ws), max_bytes: DRIVE_TEXT_MAX_BYTES })
        }`,
        (v) => {
          const result = parseDriveReadTextResponse(v, ws);
          exact(result.entry, ref);
          return result;
        },
        { signal },
      ),
    image: async (ws, entry, signal) => {
      parseDriveEntry(entry, ws);
      // Do not render active SVG or HTML as a preview.
      if (
        !/^(image\/(png|jpeg|gif|webp))$/.test(entry.content_type ?? "") ||
        entry.size === null || entry.size > DRIVE_RESPONSE_MAX_BYTES
      ) throw new DriveRequestError("limit", "not_committed");
      const res = await response(
        driveDownloadUrl(ws, entry.entry, entry.revision),
        { signal },
      );
      await checked(res);
      const bytes = await readBoundedDriveBytes(res, DRIVE_RESPONSE_MAX_BYTES);
      if (bytes.length !== entry.size) invalid();
      return new Blob([bytes], { type: entry.content_type! });
    },
    mutate: (ws, id, mutation, signal) => {
      validateMutation(mutation, ws);
      return json(
        `${base(ws)}/mutate`,
        (v) => {
          const result = parseDriveMutationResponse(v, ws, id);
          if (mutation.operation === "delete") {
            if (result.entry !== null) invalid();
          } else {
            if (result.entry === null) invalid();
            if (
              mutation.operation === "create_folder" &&
              result.entry.kind !== "folder"
            ) invalid();
            if (
              (mutation.operation === "create_text" ||
                mutation.operation === "update_text") &&
              result.entry.kind !== "file"
            ) invalid();
            if ("name" in mutation && result.entry.name !== mutation.name) {
              invalid();
            }
            if (
              "id" in mutation &&
              result.entry.entry.node_id !== mutation.id.node_id
            ) invalid();
            if (
              "parent" in mutation &&
              result.entry.parent?.node_id !== mutation.parent.node_id
            ) invalid();
          }
          return result;
        },
        {
          method: "POST",
          signal,
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ request_id: requestId(id), mutation }),
        },
        true,
      );
    },
    upload: async (ws, id, blob, target, signal) => {
      base(ws);
      requestId(id);
      if (blob.size > DRIVE_FILE_MAX_BYTES) {
        throw new DriveRequestError("limit", "not_committed");
      }
      const contentType = mime(target.content_type);
      const fields = target.operation === "create"
        ? {
          parent_id: parseDriveEntryRef(target.parent, ws).node_id,
          name: name(target.name),
        }
        : {
          id: parseDriveEntryRef(target.id, ws).node_id,
          expected_revision: parseDriveDecimal(target.expected_revision),
        };
      signal?.throwIfAborted();
      const digest = await crypto.subtle.digest(
        "SHA-256",
        await blob.arrayBuffer(),
      );
      signal?.throwIfAborted();
      const sha256 = Array.from(
        new Uint8Array(digest),
        (byte) => byte.toString(16).padStart(2, "0"),
      ).join("");
      return json(
        `${base(ws)}/upload${
          query({
            operation: target.operation,
            request_id: id,
            entry_workspace_id: ws,
            ...fields,
            content_type: contentType,
            size: blob.size,
            sha256,
          })
        }`,
        (v) => {
          const result = parseDriveMutationResponse(v, ws, id);
          if (result.entry === null || result.entry.kind !== "file") invalid();
          if (
            target.operation === "create" && result.entry.name !== target.name
          ) invalid();
          if (
            target.operation === "update" &&
            result.entry.entry.node_id !== target.id.node_id
          ) invalid();
          if (
            target.operation === "create" &&
            result.entry.parent?.node_id !== target.parent.node_id
          ) invalid();
          return result;
        },
        {
          method: "PUT",
          signal,
          headers: { "content-type": "application/octet-stream" },
          body: blob,
        },
        true,
      );
    },
    status: (ws, id, signal) =>
      json(
        `${base(ws)}/requests/${encodeURIComponent(requestId(id))}`,
        (v) => parseDriveRequestStatusResponse(v, ws, id),
        { signal },
      ),
  };
}
