export function profileSettingsHref(workspaceId: string): string {
  return `/w/${encodeURIComponent(workspaceId)}/settings/profiles`;
}

export function profileSourceTreeSettingsHref(workspaceId: string, sourceTreeId: string): string {
  return `${profileSettingsHref(workspaceId)}/trees/${encodeURIComponent(sourceTreeId)}`;
}

export function virtualProfilePathForCreate(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return "";
  if (trimmed.startsWith("project:") || trimmed.startsWith("workspace:")) return trimmed;
  if (trimmed.startsWith("profiles/")) return trimmed;
  return `profiles/${trimmed}`;
}
