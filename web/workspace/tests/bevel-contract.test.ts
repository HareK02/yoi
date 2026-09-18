// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Bevel draws a plain selectable border around semantic children", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/lib/workspace/ui/Bevel.svelte", import.meta.url),
  );
  const appCss = await Deno.readTextFile(
    new URL("../src/app.css", import.meta.url),
  );
  const designLanguage = await Deno.readTextFile(
    new URL(
      "../../../docs/development/ui-ux/design-language.md",
      import.meta.url,
    ),
  );

  for (
    const contract of [
      "children: Snippet<[]>",
      "top?: boolean",
      "right?: boolean",
      "bottom?: boolean",
      "left?: boolean",
      "data-invalid={invalid || undefined}",
      "border-style: solid",
      "border-color: var(--line-strong)",
      "border-width: 1px",
      "aria-invalid='true'",
      "forced-colors: active",
    ]
  ) {
    assert(source.includes(contract), `Bevel must preserve ${contract}`);
  }

  assert(
    source.includes("data-top={top ? 'true' : 'false'}") &&
      source.includes("data-right={right ? 'true' : 'false'}") &&
      source.includes("data-bottom={bottom ? 'true' : 'false'}") &&
      source.includes("data-left={left ? 'true' : 'false'}") &&
      source.includes("border-top-width: 0") &&
      source.includes("border-right-width: 0") &&
      source.includes("border-bottom-width: 0") &&
      source.includes("border-left-width: 0") &&
      source.includes("--bevel-top-left-radius: 0px") &&
      source.includes("--bevel-bottom-right-radius: 0px"),
    "Bevel must draw only selected sides and round only corners shared by enabled sides",
  );
  assert(
    !source.includes("BevelProfile") &&
      !source.includes("BevelDepth") &&
      !source.includes("profile") &&
      !source.includes("depth") &&
      !source.includes("pressed") &&
      !source.includes("::before") &&
      !source.includes("::after") &&
      !source.includes("gradient") &&
      !source.includes("mask") &&
      !source.includes("color-mix") &&
      !source.includes("cos(") &&
      !source.includes("pow(") &&
      !appCss.includes("--bevel-highlight") &&
      !appCss.includes("--bevel-shadow") &&
      !appCss.includes("--bevel-highlight-curve") &&
      !appCss.includes("--bevel-face-width"),
    "Bevel must remain a plain border without lighting or depth simulation",
  );
  assert(
    !source.includes("background:") &&
      !source.includes("background-color:") &&
      source.includes("button, input, select, textarea"),
    "Bevel must not own surface color and must keep semantic controls as children",
  );
  assert(
    designLanguage.includes("1pxの単色borderを描くvisual wrapper") &&
      designLanguage.includes(
        "照明、raised／inset、ridge／grooveを表現しない",
      ) &&
      designLanguage.includes("text/content areaへ適用する"),
    "The design language must define Bevel as a plain border without surface ownership",
  );
});

Deno.test("Workspace showroom exercises closed and adjacent borders", async () => {
  const workspaceSource = await Deno.readTextFile(
    new URL(
      "../src/routes/design-lab/workspace-web-ux/+page.svelte",
      import.meta.url,
    ),
  );
  const settingsSource = await Deno.readTextFile(
    new URL(
      "../src/routes/design-lab/workspace-web-ux/settings/+page.svelte",
      import.meta.url,
    ),
  );
  const showroomCss = await Deno.readTextFile(
    new URL(
      "../src/routes/design-lab/workspace-web-ux/showroom.css",
      import.meta.url,
    ),
  );
  const tactileGroupRule = showroomCss.match(
    /\.tactile-group \{([\s\S]*?)\n  \}/,
  )?.[1];

  assert(
    workspaceSource.includes('<div class="border-matrix">') &&
      workspaceSource.includes("right={false} bottom={false}") &&
      workspaceSource.includes("top={false} left={false}") &&
      workspaceSource.includes("Closed border") &&
      workspaceSource.includes("Read-only border") &&
      !workspaceSource.includes("profile=") &&
      !workspaceSource.includes("depth=") &&
      !workspaceSource.includes("<Bevel pressed"),
    "Workspace showroom must demonstrate closed and adjacent plain borders",
  );
  assert(
    tactileGroupRule && !tactileGroupRule.includes("background"),
    "Showroom bordered items must leave their content area background unset",
  );
  assert(
    settingsSource.includes("<Bevel fill>") &&
      settingsSource.includes("<Bevel fill invalid>") &&
      settingsSource.includes('aria-invalid="true"') &&
      !settingsSource.includes("profile=") &&
      !settingsSource.includes("depth="),
    "Settings showroom must demonstrate plain and invalid borders",
  );
});
