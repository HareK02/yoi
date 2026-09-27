import { error } from "@sveltejs/kit";
import { readBoundedJson } from "./http.ts";
import {
  REPOSITORY_ACCESS_MAX_RESPONSE_BYTES,
  RepositoryAccessSchemaError,
} from "./repository-access.ts";

export type RepositoryAccessSectionResult<T> = {
  data: T | null;
  error: string | null;
};

function sectionErrorMessage(cause: unknown, label: string): string {
  if (cause && typeof cause === "object" && "body" in cause) {
    const body = (cause as { body?: unknown }).body;
    if (body && typeof body === "object" && "message" in body) {
      const message = (body as { message?: unknown }).message;
      if (typeof message === "string") return message;
    }
  }
  return `${label} could not be loaded.`;
}

function httpErrorStatus(cause: unknown): number | null {
  if (!cause || typeof cause !== "object" || !("status" in cause)) return null;
  const status = (cause as { status?: unknown }).status;
  return typeof status === "number" ? status : null;
}

export async function loadRepositoryAccessPermissionGate<T>(
  fetcher: typeof fetch,
  path: string,
  parse: (value: unknown) => T,
  label: string,
): Promise<RepositoryAccessSectionResult<T>> {
  try {
    return {
      data: await loadRepositoryAccessJson(fetcher, path, parse),
      error: null,
    };
  } catch (cause) {
    if (httpErrorStatus(cause) === 403) throw cause;
    return { data: null, error: sectionErrorMessage(cause, label) };
  }
}

export async function loadRepositoryAccessSection<T>(
  fetcher: typeof fetch,
  path: string,
  parse: (value: unknown) => T,
  label: string,
): Promise<RepositoryAccessSectionResult<T>> {
  try {
    return {
      data: await loadRepositoryAccessJson(fetcher, path, parse),
      error: null,
    };
  } catch (cause) {
    return { data: null, error: sectionErrorMessage(cause, label) };
  }
}

export async function loadRepositoryAccessJson<T>(
  fetcher: typeof fetch,
  path: string,
  parse: (value: unknown) => T,
): Promise<T> {
  let response: Response;
  try {
    response = await fetcher(path, { headers: { accept: "application/json" } });
  } catch {
    error(503, { message: "Repository Access is temporarily unavailable." });
  }

  if (response.status === 401 || response.status === 403) {
    error(403, {
      message: "Repository Access is unavailable for this account.",
    });
  }
  if (!response.ok) {
    error(502, {
      message:
        `Repository Access request failed with status ${response.status}.`,
    });
  }

  let payload: unknown;
  try {
    payload = await readBoundedJson(
      response,
      REPOSITORY_ACCESS_MAX_RESPONSE_BYTES,
    );
  } catch {
    error(502, {
      message: "Repository Access returned an invalid JSON response.",
    });
  }

  try {
    return parse(payload);
  } catch (cause) {
    if (cause instanceof RepositoryAccessSchemaError) {
      error(502, { message: cause.message });
    }
    throw cause;
  }
}
