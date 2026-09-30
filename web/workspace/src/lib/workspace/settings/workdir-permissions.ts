import type { ExternalWorkdirPermissions } from "$lib/generated/workdir-api";

/** Render an effective attachment/session capability intersection without widening it. */
export function formatWorkdirPermissions(
  permissions: ExternalWorkdirPermissions,
): string {
  const categories: string[] = [];
  if (permissions.read) categories.push("READ");
  if (permissions.write) categories.push("WRITE");
  if (permissions.command) categories.push("COMMAND");
  return categories.length > 0 ? categories.join(" · ") : "NONE";
}

/** Render one validated External grant level with its inherited authority. */
export function formatExternalGrantPermissionLevel(
  permissions: ExternalWorkdirPermissions,
): string {
  if (permissions.read && permissions.write && permissions.command) {
    return "COMMAND (includes WRITE + READ)";
  }
  if (permissions.read && permissions.write && !permissions.command) {
    return "WRITE (includes READ)";
  }
  if (permissions.read && !permissions.write && !permissions.command) {
    return "READ";
  }
  throw new Error(
    "External Workdir grant has a non-hierarchical permission set",
  );
}
