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

Deno.test("settings replaces only WorkspaceSidebar content", async () => {
  const layoutUrl = new URL(
    "../../../routes/w/[workspaceId]/settings/+layout.svelte",
    import.meta.url,
  );
  const workspaceLayoutUrl = new URL(
    "../../../routes/w/[workspaceId]/+layout.svelte",
    import.meta.url,
  );
  const workspaceSidebarUrl = new URL(
    "./WorkspaceSidebar.svelte",
    import.meta.url,
  );
  const settingsContentUrl = new URL(
    "./SettingsSidebarContent.svelte",
    import.meta.url,
  );
  const [layout, workspaceLayout, workspaceSidebar, settingsContent] =
    await Promise.all([
      Deno.readTextFile(layoutUrl),
      Deno.readTextFile(workspaceLayoutUrl),
      Deno.readTextFile(workspaceSidebarUrl),
      Deno.readTextFile(settingsContentUrl),
    ]);

  assert(
    layout.includes(
      "<WorkspaceSidebarContentOverride content={settingsSidebarContent} />",
    ),
    "settings layout should override the WorkspaceSidebar content slot",
  );
  assert(
    !layout.includes("<SidebarOverride") && !layout.includes("settings-nav"),
    "settings layout should not replace the whole sidebar or retain inline navigation",
  );
  assert(
    workspaceLayout.includes(
      "registerContent: sidebarContentOverrides.register",
    ) &&
      workspaceLayout.includes("content={sidebarContent}"),
    "workspace layout should provide and project the active child content",
  );
  assert(
    workspaceSidebar.includes("<WorkspaceSwitcher") &&
      workspaceSidebar.indexOf("<WorkspaceSwitcher") <
        workspaceSidebar.indexOf("{#if content}") &&
      workspaceSidebar.includes("{@render content()}"),
    "WorkspaceSidebar should retain its header and render child content below it",
  );
  assert(
    settingsContent.includes("SETTINGS_SECTIONS") &&
      settingsContent.includes('aria-label="Settings sections"'),
    "SettingsSidebarContent should render the authoritative settings section catalog",
  );
});
