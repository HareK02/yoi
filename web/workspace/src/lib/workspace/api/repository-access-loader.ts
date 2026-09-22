import { error } from "@sveltejs/kit";
import { readBoundedJson } from "./http.ts";
import {
  REPOSITORY_ACCESS_MAX_RESPONSE_BYTES,
  RepositoryAccessSchemaError,
} from "./repository-access.ts";

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
