declare const Deno: {
  test(name: string, fn: () => Promise<void> | void): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const switcherSource = await Deno.readTextFile(
  new URL("./WorkspaceSwitcher.svelte", import.meta.url),
);
const sidebarSource = await Deno.readTextFile(
  new URL("./WorkspaceSidebar.svelte", import.meta.url),
);
const sidebarStyles = await Deno.readTextFile(
  new URL("./sidebar.css", import.meta.url),
);
const headerSource = await Deno.readTextFile(
  new URL("../header/WorkspaceBreadcrumbs.svelte", import.meta.url),
);
const workspaceLayoutSource = await Deno.readTextFile(
  new URL("../../../routes/w/[workspaceId]/+layout.svelte", import.meta.url),
);

Deno.test("workspace name opens the settings and workspace menu", () => {
  assert(
    switcherSource.includes('class="workspace-menu-trigger"'),
    "missing name trigger",
  );
  assert(
    switcherSource.includes('aria-haspopup="menu"'),
    "trigger is not a menu button",
  );
  assert(
    switcherSource.includes("<span>Settings</span>"),
    "missing Settings action",
  );
  assert(
    switcherSource.includes("<span>Workspaces</span>"),
    "missing Workspaces heading",
  );
  assert(
    switcherSource.includes('aria-label="Create Workspace"'),
    "missing create action",
  );
  assert(
    switcherSource.includes("currentWorkspaceName"),
    "trigger does not use the name",
  );
  assert(
    switcherSource.includes("workspace-menu-popover-${variant}"),
    "sidebar and header instances should use distinct menu ids",
  );
  assert(!switcherSource.includes("<select"), "legacy select switcher remains");
});

Deno.test("workspace menu lists catalog entries and marks the current workspace", () => {
  assert(
    switcherSource.includes("listWorkspaces(fetch)"),
    "catalog is not loaded",
  );
  assert(
    switcherSource.includes("{#each menuWorkspaces as workspace"),
    "Workspace catalog is not rendered",
  );
  assert(
    switcherSource.includes("aria-current="),
    "current Workspace is not identified",
  );
  assert(
    switcherSource.includes("workspace.display_name"),
    "Workspace name is not rendered",
  );
});

Deno.test("workspace header prefixes breadcrumbs with the same Workspace menu", () => {
  assert(
    headerSource.includes("<WorkspaceSwitcher") &&
      headerSource.includes('variant="header"') &&
      headerSource.includes("currentWorkspaceName"),
    "header should render the shared Workspace selector before breadcrumbs",
  );
  assert(
    headerSource.indexOf("<WorkspaceSwitcher") <
      headerSource.indexOf('<nav class="workspace-breadcrumbs"'),
    "Workspace selector should precede the breadcrumb trail",
  );
  assert(
    workspaceLayoutSource.includes("workspace={data.workspace ?? null}") &&
      workspaceLayoutSource.includes(
        "workspaceError={data.workspaceError ?? null}",
      ),
    "Workspace layout should pass the authoritative Workspace name to the header selector",
  );
});

Deno.test("WorkspaceSidebar exposes icon-only home and settings shortcuts", () => {
  assert(
    sidebarSource.includes('aria-label="Workspace shortcuts"') &&
      sidebarSource.includes('aria-label="Workspace home"') &&
      sidebarSource.includes('aria-label="Workspace settings"') &&
      sidebarSource.includes("workspaceRoute(workspaceId)") &&
      sidebarSource.includes("workspaceRoute(workspaceId, '/settings')") &&
      sidebarStyles.includes(".workspace-sidebar-shortcut.active"),
    "Workspace Sidebar should expose accessible Home and Settings icon links",
  );
  assert(
    !headerSource.includes("workspace-sidebar-shortcut"),
    "Workspace shortcuts should remain specific to the Sidebar",
  );
});

Deno.test("Workspace selector remains in the Header only", () => {
  assert(
    !sidebarSource.includes("<WorkspaceSwitcher"),
    "Workspace Sidebar should not render the Workspace selector",
  );
  assert(
    headerSource.includes("<WorkspaceSwitcher"),
    "Header should retain the shared Workspace selector",
  );
  assert(
    !sidebarSource.includes('class="sidebar-actions-row"'),
    "old action row remains",
  );
  assert(
    !sidebarSource.includes("RepositoriesNavSection"),
    "Repositories should not be rendered in the Workspace sidebar",
  );
});
