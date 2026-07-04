import type { Diagnostic, RepositoryListResponse } from "./types";

export type RepositoryNavItem = {
  id: string;
  title: string;
  href: string;
  meta: string;
  active: boolean;
};

export type RepositoryNavProjection = {
  count: number;
  items: RepositoryNavItem[];
  diagnostics: Diagnostic[];
};

export function projectRepositoryNav(
  repositories: RepositoryListResponse | null,
  currentPath = "/",
): RepositoryNavProjection {
  const summaries = repositories?.items ?? [];
  return {
    count: summaries.length,
    diagnostics: repositories?.diagnostics ?? [],
    items: summaries.map((repository) => {
      const href = `/repositories/${encodeURIComponent(repository.id)}`;
      return {
        id: repository.id,
        title: repository.display_name || repository.id,
        href,
        meta: `${repository.provider} repository · read-only`,
        active: currentPath === href || currentPath.startsWith(`${href}/`),
      };
    }),
  };
}
