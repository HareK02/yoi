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

  if (
    segments[0] === "runtimes" &&
    segments[2] === "workers" &&
    segments[3]
  ) {
    return [
      { label: "workers", href: `${workspaceRoot}/workers` },
      { label: context.workerName?.trim() || segments[3] },
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
