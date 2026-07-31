import type { WorkingDirectorySummary } from "../sidebar/types.ts";

export function formatCurrentWorkdirRevision(
  workdir: WorkingDirectorySummary,
  repositoryProvider: string | null | undefined,
): string {
  const selector = workdir.current_selector?.trim() || null;
  const reference = workdir.current_ref?.trim() || null;

  if (repositoryProvider?.toLowerCase() === "git") {
    const hash = reference ? shortGitHash(reference) : null;
    if (selector && hash) return `${selector}@${hash}`;
    return selector ?? hash ?? "—";
  }

  if (selector && reference) return `${selector} · ${reference}`;
  return selector ?? reference ?? "—";
}

function shortGitHash(reference: string): string {
  return reference.length > 12 ? reference.slice(0, 12) : reference;
}
