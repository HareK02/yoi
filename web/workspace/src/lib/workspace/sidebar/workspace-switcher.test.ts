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

Deno.test("workspace sidebar uses the workspace name menu as its header", () => {
  assert(
    sidebarSource.includes("<WorkspaceSwitcher"),
    "sidebar omits the menu",
  );
  assert(
    sidebarSource.includes("currentWorkspaceName={workspace.display_name}"),
    "sidebar does not pass the current Workspace name",
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
