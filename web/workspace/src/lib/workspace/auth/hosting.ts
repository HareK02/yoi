import type {
  HostingAccess,
  HostingSettings,
} from "#lib/generated/auth-api.ts";
import { readBoundedAuthResponseJson } from "./api.ts";

export class HostingRequestError extends Error {
  constructor(readonly status: number) {
    super(`Hosting request failed (${status}).`);
  }
}

function booleanRecord(value: unknown, key: string): Record<string, boolean> {
  if (
    value === null || typeof value !== "object" || Array.isArray(value) ||
    Object.keys(value).length !== 1 ||
    !Object.hasOwn(value, key) ||
    typeof (value as Record<string, unknown>)[key] !== "boolean"
  ) {
    throw new Error(
      `Invalid hosting response: expected only ${key} as a boolean.`,
    );
  }
  return value as Record<string, boolean>;
}

export function parseHostingSettings(value: unknown): HostingSettings {
  return {
    self_registration_enabled: booleanRecord(value, "self_registration_enabled")
      .self_registration_enabled,
  };
}

export function parseHostingAccess(value: unknown): HostingAccess {
  return { is_admin: booleanRecord(value, "is_admin").is_admin };
}

async function requestJson(path: string, init?: RequestInit): Promise<unknown> {
  const response = await fetch(path, {
    credentials: "same-origin",
    cache: "no-store",
    ...init,
    headers: { "content-type": "application/json", ...init?.headers },
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new HostingRequestError(response.status);
  }
  return await readBoundedAuthResponseJson(response);
}

export async function loadRegistrationPolicy(): Promise<HostingSettings> {
  return parseHostingSettings(
    await requestJson("/api/auth/registration-policy"),
  );
}

export async function loadHostingAccess(): Promise<HostingAccess> {
  return parseHostingAccess(await requestJson("/api/hosting/access"));
}

export async function loadHostingSettings(): Promise<HostingSettings> {
  return parseHostingSettings(await requestJson("/api/hosting/settings"));
}

export async function saveHostingSettings(
  settings: HostingSettings,
): Promise<HostingSettings> {
  return parseHostingSettings(
    await requestJson("/api/hosting/settings", {
      method: "PUT",
      body: JSON.stringify(parseHostingSettings(settings)),
    }),
  );
}

// Session変更は再取得の契機だけに使い、権限そのものはaccess endpointで確認する。
export const SESSION_CHANGED_EVENT = "yoi:browser-session-changed";
export function notifySessionChanged(): void {
  window.dispatchEvent(new Event(SESSION_CHANGED_EVENT));
}
