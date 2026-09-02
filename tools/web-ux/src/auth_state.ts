import { writePrivateJson } from "./artifacts.ts";

export type AuthStateMetadata = {
  schemaVersion: 1;
  personaId: string;
  baseOrigin: string;
  createdAt: string;
  expiresAt: string;
};

export function authMetadataPath(storageStatePath: string): string {
  return `${storageStatePath}.meta.json`;
}

function baseOrigin(baseUrl: string): string {
  return new URL(baseUrl).origin;
}

export async function writeAuthMetadata(
  storageStatePath: string,
  personaId: string,
  baseUrl: string,
  expiresInHours: number,
): Promise<void> {
  if (!Number.isFinite(expiresInHours) || expiresInHours <= 0) {
    throw new Error("auth state expiry must be a positive number of hours");
  }
  const createdAt = new Date();
  const metadata: AuthStateMetadata = {
    schemaVersion: 1,
    personaId,
    baseOrigin: baseOrigin(baseUrl),
    createdAt: createdAt.toISOString(),
    expiresAt: new Date(createdAt.getTime() + expiresInHours * 60 * 60 * 1000).toISOString(),
  };
  await writePrivateJson(authMetadataPath(storageStatePath), metadata);
}

export async function validateAuthState(
  storageStatePath: string,
  personaId: string,
  baseUrl: string,
  now = new Date(),
): Promise<void> {
  await Deno.stat(storageStatePath);
  let parsed: unknown;
  try {
    parsed = JSON.parse(await Deno.readTextFile(authMetadataPath(storageStatePath)));
  } catch (error) {
    throw new Error(
      `auth state metadata is missing or invalid for ${personaId}; run the auth command again: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error(`auth state metadata is invalid for ${personaId}`);
  }
  const metadata = parsed as Partial<AuthStateMetadata>;
  if (metadata.schemaVersion !== 1 || metadata.personaId !== personaId) {
    throw new Error(`auth state metadata does not match persona ${personaId}`);
  }
  if (metadata.baseOrigin !== baseOrigin(baseUrl)) {
    throw new Error(
      `auth state for ${personaId} belongs to ${metadata.baseOrigin ?? "an unknown origin"}, not ${
        baseOrigin(baseUrl)
      }`,
    );
  }
  const expiresAt = Date.parse(metadata.expiresAt ?? "");
  if (!Number.isFinite(expiresAt)) throw new Error(`auth state expiry is invalid for ${personaId}`);
  if (expiresAt <= now.getTime()) {
    throw new Error(
      `auth state expired for ${personaId} at ${metadata.expiresAt}; run the auth command again`,
    );
  }
}

export async function deleteAuthState(storageStatePath: string): Promise<void> {
  for (const path of [storageStatePath, authMetadataPath(storageStatePath)]) {
    try {
      await Deno.remove(path);
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
    }
  }
}
