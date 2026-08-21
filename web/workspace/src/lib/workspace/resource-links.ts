const RESOURCE_KEY_PATTERN = /^(T|O|W)-(\d+)/;

export function resourceKey(reference: string): string {
  const match = RESOURCE_KEY_PATTERN.exec(reference);
  return match ? `${match[1]}-${match[2]}` : reference;
}

export function slugifyResourceTitle(title: string): string {
  const slug = title
    .normalize("NFKD")
    .replace(/\p{Mark}+/gu, "")
    .toLocaleLowerCase("en-US")
    .replace(/[^\p{Letter}\p{Number}]+/gu, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 80)
    .replace(/-+$/g, "");
  return slug || "resource";
}

export function canonicalResourceReference(
  resourceKey: string,
  title: string,
): string {
  return `${resourceKey}-${slugifyResourceTitle(title)}`;
}

export function ticketHref(
  workspaceId: string,
  ticket: { resource_key: string; title: string },
): string {
  return `/w/${encodeURIComponent(workspaceId)}/tickets/${encodeURIComponent(canonicalResourceReference(ticket.resource_key, ticket.title))}`;
}

export function objectiveHref(
  workspaceId: string,
  objective: { resource_key: string; title: string },
): string {
  return `/w/${encodeURIComponent(workspaceId)}/objectives/${encodeURIComponent(canonicalResourceReference(objective.resource_key, objective.title))}`;
}

export function workerHref(
  workspaceId: string,
  worker: { resource_key: string; display_name: string },
): string {
  const reference = canonicalResourceReference(worker.resource_key, worker.display_name);
  return `/w/${encodeURIComponent(workspaceId)}/workers/${encodeURIComponent(reference)}`;
}
