import type { ExternalWorkdirPermissions } from "$lib/generated/workdir-api";

/** Render independent Workdir authority categories in a stable, non-conflating order. */
export function formatWorkdirPermissions(
  permissions: ExternalWorkdirPermissions,
): string {
  const categories: string[] = [];
  if (permissions.read) categories.push("READ");
  if (permissions.write) categories.push("WRITE");
  if (permissions.command) categories.push("COMMAND");
  return categories.length > 0 ? categories.join(" · ") : "NONE";
}
