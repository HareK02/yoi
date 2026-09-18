// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("BevelLine exposes a semantic directional separator", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/lib/workspace/ui/BevelLine.svelte", import.meta.url),
  );

  for (
    const contract of [
      "export type BevelLineDirection = 'x' | 'y'",
      "length?: string",
      "as?: 'span' | 'div'",
      "role={decorative ? undefined : 'separator'}",
      "'horizontal' : 'vertical'",
      "style:width={direction === 'x' ? length : undefined}",
      "style:height={direction === 'y' ? length : undefined}",
      "pointer-events: none",
      "forced-colors: active",
    ]
  ) {
    assert(source.includes(contract), `BevelLine must preserve ${contract}`);
  }

  assert(
    source.includes("border-color: var(--line-strong)") &&
      source.includes("border-top-style: solid") &&
      source.includes("border-top-width: 1px") &&
      source.includes("border-left-style: solid") &&
      source.includes("border-left-width: 1px") &&
      !source.includes("background:") &&
      !source.includes("gradient") &&
      !source.includes("color-mix") &&
      !source.includes("BevelLineDepth") &&
      !source.includes("depth"),
    "BevelLine must remain a plain one-pixel directional border",
  );
  assert(
    !source.includes("padding-inline") &&
      !source.includes("padding-block") &&
      !source.includes("margin-inline") &&
      !source.includes("margin-block"),
    "BevelLine must fill its parent layer without independent endpoint spacing",
  );
});

Deno.test("Workspace showroom demonstrates both BevelLine directions", async () => {
  const source = await Deno.readTextFile(
    new URL(
      "../src/routes/design-lab/workspace-web-ux/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    source.includes('direction="x" length="100%"') &&
      source.includes('direction="y" length="72px"') &&
      source.includes('direction="x" length="160px"') &&
      !source.includes("depth=") &&
      !source.includes("weight="),
    "Workspace showroom must demonstrate horizontal and vertical plain separators",
  );
});

Deno.test("Shell regions use selected Bevel edges while open separators use BevelLine", async () => {
  const rootLayout = await Deno.readTextFile(
    new URL("../src/routes/+layout.svelte", import.meta.url),
  );
  const sidebarFrame = await Deno.readTextFile(
    new URL(
      "../src/lib/workspace/sidebar/SidebarFrame.svelte",
      import.meta.url,
    ),
  );
  const sidebarCss = await Deno.readTextFile(
    new URL("../src/lib/workspace/sidebar/sidebar.css", import.meta.url),
  );
  const showroomCss = await Deno.readTextFile(
    new URL(
      "../src/routes/design-lab/workspace-web-ux/showroom.css",
      import.meta.url,
    ),
  );
  const designLanguage = await Deno.readTextFile(
    new URL(
      "../../../docs/development/ui-ux/design-language.md",
      import.meta.url,
    ),
  );

  assert(
    rootLayout.includes('class="app-shell__topbar-bevel"') &&
      rootLayout.includes("top={false} right={false} left={false}") &&
      rootLayout.includes("bind:folded={sidebarFolded}") &&
      rootLayout.includes("app-shell__mobile-sidebar-toggle") &&
      rootLayout.includes("class:sidebar-open={!sidebarFolded}") &&
      !rootLayout.includes("depth=") &&
      !rootLayout.includes("BevelLine") &&
      sidebarFrame.includes('class="sidebar-frame__bevel"') &&
      sidebarFrame.includes("top={false} bottom={false} left={false}") &&
      sidebarFrame.includes("$bindable(false)") &&
      !sidebarFrame.includes("depth=") &&
      !sidebarFrame.includes("BevelLine") &&
      sidebarCss.includes(".sidebar-frame.folded") &&
      sidebarCss.includes("display: none") &&
      sidebarCss.includes("display: grid"),
    "Desktop shell boundaries must use one selected border edge and Mobile must share Header fold state",
  );
  assert(
    designLanguage.includes(
      "Headerのbottom edgeとSidebarのright edgeだけを有効",
    ) &&
      designLanguage.includes("Mobileのfold controlはHeaderに置く") &&
      designLanguage.includes("main contentと同時表示しない") &&
      designLanguage.includes("同じlayerではheading、本文、Lineの端を揃え") &&
      designLanguage.includes(
        "親layoutがそのlayer全体へinline方向の余白を与え",
      ) &&
      showroomCss.includes(".resource-list {") &&
      showroomCss.includes("margin-inline: var(--space-2)"),
    "The design language and showroom must preserve adjacent area and line hierarchy",
  );
  assert(
    sidebarCss.includes(".section-action {") &&
      sidebarCss.includes("padding: var(--space-1) var(--space-2)") &&
      sidebarCss.includes("font-size: var(--font-size-compact)") &&
      sidebarCss.includes("line-height: var(--line-height-compact)") &&
      !showroomCss.includes(".workspace-sidebar .section-action"),
    "Workspace Sidebar must own the Workers section action spacing used by the showroom",
  );
  assert(
    !sidebarCss.includes("border-right:") &&
      !sidebarCss.includes("border-bottom:") &&
      !showroomCss.includes("border-top: 1px solid") &&
      !showroomCss.includes("border-bottom: 1px solid"),
    "Shell and showroom structural boundaries must use shared Bevel components",
  );
});
