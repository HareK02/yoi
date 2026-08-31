import { createOverrideStack } from "./override-stack.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: string | URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals<T>(actual: T, expected: T, message: string): void {
  if (!Object.is(actual, expected)) {
    throw new Error(
      `${message}: expected ${String(expected)}, received ${String(actual)}`,
    );
  }
}

Deno.test("nested sidebar cleanup restores the parent override", () => {
  let active: string | null = null;
  const stack = createOverrideStack<string>((value) => {
    active = value;
  });

  const clearWorkspace = stack.register("workspace");
  assertEquals(active, "workspace", "workspace sidebar should become active");

  const clearSettings = stack.register("settings");
  assertEquals(
    active,
    "settings",
    "child settings sidebar should override its parent",
  );

  clearSettings();
  assertEquals(
    active,
    "workspace",
    "removing the child should restore its parent",
  );

  clearWorkspace();
  assertEquals(
    active,
    null,
    "removing the final override should restore the global sidebar",
  );
});

Deno.test("sidebar disposers remove only their own registration", () => {
  let active: string | null = null;
  const stack = createOverrideStack<string>((value) => {
    active = value;
  });

  const clearWorkspace = stack.register("workspace");
  const clearSettings = stack.register("settings");

  clearWorkspace();
  assertEquals(
    active,
    "settings",
    "removing an inactive parent should preserve the active child",
  );

  clearWorkspace();
  assertEquals(active, "settings", "a disposer should be idempotent");

  clearSettings();
  assertEquals(
    active,
    null,
    "removing the remaining child should empty the stack",
  );
});

Deno.test("Global, Workspace, and Settings use one recursive sidebar slot contract", async () => {
  const rootLayoutUrl = new URL(
    "../../../routes/+layout.svelte",
    import.meta.url,
  );
  const workspaceLayoutUrl = new URL(
    "../../../routes/w/[workspaceId]/+layout.svelte",
    import.meta.url,
  );
  const settingsLayoutUrl = new URL(
    "../../../routes/w/[workspaceId]/settings/+layout.svelte",
    import.meta.url,
  );
  const globalSidebarUrl = new URL("./GlobalSidebar.svelte", import.meta.url);
  const workspaceSidebarUrl = new URL(
    "./WorkspaceSidebar.svelte",
    import.meta.url,
  );
  const settingsSidebarUrl = new URL(
    "./SettingsSidebar.svelte",
    import.meta.url,
  );
  const settingsErrorUrl = new URL(
    "../../../routes/w/[workspaceId]/settings/+error.svelte",
    import.meta.url,
  );
  const [
    rootLayout,
    workspaceLayout,
    settingsLayout,
    globalSidebar,
    workspaceSidebar,
    settingsSidebar,
    settingsError,
  ] = await Promise.all([
    Deno.readTextFile(rootLayoutUrl),
    Deno.readTextFile(workspaceLayoutUrl),
    Deno.readTextFile(settingsLayoutUrl),
    Deno.readTextFile(globalSidebarUrl),
    Deno.readTextFile(workspaceSidebarUrl),
    Deno.readTextFile(settingsSidebarUrl),
    Deno.readTextFile(settingsErrorUrl),
  ]);

  assert(
    rootLayout.includes("<GlobalSidebar") &&
      rootLayout.includes("content={sidebar}"),
    "root layout should always render GlobalSidebar as the root slot owner",
  );
  for (
    const [name, layout] of [
      ["Workspace", workspaceLayout],
      ["Settings", settingsLayout],
    ] as const
  ) {
    assert(
      layout.includes(
        "const parentSidebarController = getSidebarController();",
      ) &&
        layout.includes("setContext<SidebarController>(SIDEBAR_CONTEXT") &&
        layout.includes("controller={parentSidebarController}"),
      `${name} layout should register with its parent and provide the same slot contract to children`,
    );
  }
  assert(
    !workspaceLayout.includes("WORKSPACE_SIDEBAR_CONTENT_CONTEXT") &&
      !settingsLayout.includes("WorkspaceSidebarContentOverride"),
    "recursive slots should not retain Workspace-specific context or override components",
  );
  assert(
    globalSidebar.includes("{@render content()}") &&
      workspaceSidebar.includes("{@render content()}") &&
      settingsSidebar.includes("{@render content()}"),
    "every sidebar layer should render its child through the same content contract",
  );
  assert(
    workspaceSidebar.includes('aria-label="Workspace shortcuts"') &&
      workspaceSidebar.indexOf('aria-label="Workspace shortcuts"') <
        workspaceSidebar.indexOf("{#if content}"),
    "WorkspaceSidebar should keep its shortcuts above the recursive child slot",
  );
  assert(
    settingsSidebar.includes("SETTINGS_SECTIONS") &&
      settingsSidebar.includes('aria-label="Settings sections"'),
    "SettingsSidebar should render the authoritative settings catalog as its fallback",
  );
  assert(
    settingsError.includes("This settings page could not be loaded") &&
      settingsError.includes("Back to Settings"),
    "settings load failures should stay inside the Settings layout and preserve its sidebar",
  );
});
