import { subjectHref, subjectsHref } from "#lib/workspace/subjektiv/routes.ts";

export type WorkspaceBreadcrumb = {
  label: string;
  href?: string;
};

export type WorkspaceBreadcrumbContext = {
  workerName?: string | null;
};

export function buildWorkspaceBreadcrumbs(
  pathname: string,
  workspaceId: string,
  context: WorkspaceBreadcrumbContext = {},
): WorkspaceBreadcrumb[] {
  const workspaceRoot = `/w/${encodeURIComponent(workspaceId)}`;
  const prefix = `${workspaceRoot}/`;
  if (pathname === workspaceRoot || pathname === `${workspaceRoot}/`) return [];
  if (!pathname.startsWith(prefix)) return [];

  const segments = pathname
    .slice(prefix.length)
    .split("/")
    .filter(Boolean)
    .map(decodeURIComponent);

  if (segments[0] === "memory" && segments[1] === "staging") {
    return [{ label: "Memory" }, { label: "Staging" }];
  }

  if (segments[0] === "subjektiv") {
    const crumbs: WorkspaceBreadcrumb[] = [{ label: "Subjects" }];
    if (!segments[1]) return crumbs;
    crumbs[0].href = subjectsHref(workspaceId);
    crumbs.push({ label: segments[1] });
    if (!segments[2]) return crumbs;
    crumbs[1].href = subjectHref(workspaceId, segments[1]);
    // memoriesは詳細URLのnamespaceであり、存在しない一覧へリンクしない。
    if (segments[2] === "memories" && segments[3]) {
      crumbs.push({ label: segments[3] });
    } else {
      const label = {
        permissions: "Operation permissions",
        journal: "Journal",
        chat: "Conversation",
      }[segments[2]];
      crumbs.push({ label: label ?? segments[2] });
    }
    return crumbs;
  }

  if (
    segments[0] === "workers" &&
    segments[1] &&
    segments[2] === "console"
  ) {
    return [
      { label: "workers", href: `${workspaceRoot}/workers` },
      { label: context.workerName?.trim() || segments[1] },
    ];
  }

  if (
    segments[0] === "settings" && segments[1] === "profiles" &&
    segments[2] === "trees"
  ) {
    return [
      { label: "settings", href: `${workspaceRoot}/settings` },
      { label: "profiles", href: `${workspaceRoot}/settings/profiles` },
      { label: segments[3] || "trees" },
    ];
  }

  if (
    segments[0] === "settings" &&
    segments[1] === "runtimes" &&
    segments[2] &&
    segments[3] === "workdirs"
  ) {
    return [
      { label: "settings", href: `${workspaceRoot}/settings` },
      { label: "runtimes", href: `${workspaceRoot}/settings/runtimes` },
      { label: segments[2] },
      { label: "workdirs" },
    ];
  }

  return segments.map((segment, index) => {
    const isCurrent = index === segments.length - 1;
    const href = isCurrent || (segments[0] === "repositories" && index === 0)
      ? undefined
      : `${workspaceRoot}/${
        segments.slice(0, index + 1).map(encodeURIComponent).join("/")
      }`;
    return {
      label: segment,
      href,
    };
  });
}
