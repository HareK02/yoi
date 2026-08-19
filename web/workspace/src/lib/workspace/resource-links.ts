const HUMAN_KEY_PATTERN = /^(T|O|W)-(\d+)/;

export function resourceHumanKey(reference: string): string {
  const match = HUMAN_KEY_PATTERN.exec(reference);
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
  humanKey: string,
  title: string,
): string {
  return `${humanKey}-${slugifyResourceTitle(title)}`;
}

export function ticketHref(
  workspaceId: string,
  ticket: { human_key: string; title: string },
): string {
  return `/w/${encodeURIComponent(workspaceId)}/tickets/${encodeURIComponent(canonicalResourceReference(ticket.human_key, ticket.title))}`;
}

export function objectiveHref(
  workspaceId: string,
  objective: { human_key: string; title: string },
): string {
  return `/w/${encodeURIComponent(workspaceId)}/objectives/${encodeURIComponent(canonicalResourceReference(objective.human_key, objective.title))}`;
}

export function workerHref(
  workspaceId: string,
  worker: { human_key?: string; display_name: string; worker_id: string },
): string {
  const reference = worker.human_key
    ? canonicalResourceReference(worker.human_key, worker.display_name)
    : worker.worker_id;
  return `/w/${encodeURIComponent(workspaceId)}/workers/${encodeURIComponent(reference)}`;
}
