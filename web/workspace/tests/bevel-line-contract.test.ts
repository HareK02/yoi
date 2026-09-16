// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("BevelLine exposes a semantic directional separator", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/lib/workspace/ui/BevelLine.svelte", import.meta.url),
  );
  const appCss = await Deno.readTextFile(
    new URL("../src/app.css", import.meta.url),
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
    appCss.includes("--bevel-face-width: 1px") &&
      source.includes("--bevel-line-face-width: var(--bevel-face-width)") &&
      source.includes(
        "--bevel-line-width: calc(var(--bevel-line-face-width) * 2)",
      ) &&
      !source.includes("BevelLineWeight") &&
      !source.includes("data-weight"),
    "BevelLine must compose a fixed 2px ridge from two project-wide 1px faces",
  );
  assert(
    !source.includes("--bevel-line-inset") &&
      !source.includes("padding-inline") &&
      !source.includes("padding-block"),
    "BevelLine must fill its parent layer without independent endpoint spacing",
  );
  assert(
    source.includes("background: var(--line-strong)") &&
      source.includes("@supports") &&
      source.includes("linear-gradient(to bottom") &&
      source.includes("linear-gradient(to right"),
    "BevelLine must retain a plain fallback and directional enhanced edges",
  );
  assert(
    source.includes("var(--bevel-highlight)") &&
      source.includes("var(--bevel-shadow)") &&
      !source.includes("surface"),
    "BevelLine must use the shared highlight and shadow palette without a surface color",
  );
  assert(
    source.includes("calc(max(cos(180deg - var(--_L)), 0) * 100%)") &&
      source.includes("var(--_s6) 90deg") &&
      source.includes("var(--_s6) 270deg") &&
      !source.includes("+ 1) * 50%"),
    "BevelLine must clamp unlit faces to the configured shadow without ambient light",
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
      source.includes('direction="x" length="160px" depth="inset"') &&
      !source.includes("weight="),
    "Workspace showroom must demonstrate horizontal, vertical, and inset BevelLine states",
  );
});

Deno.test("Shell regions use single-edge Bevel while open separators use BevelLine", async () => {
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
      rootLayout.includes('depth="inset"') &&
      rootLayout.includes("top={false} right={false} left={false}") &&
      rootLayout.includes("bind:folded={sidebarFolded}") &&
      rootLayout.includes("app-shell__mobile-sidebar-toggle") &&
      rootLayout.includes("class:sidebar-open={!sidebarFolded}") &&
      !rootLayout.includes("BevelLine") &&
      sidebarFrame.includes('class="sidebar-frame__bevel"') &&
      sidebarFrame.includes('depth="inset"') &&
      sidebarFrame.includes("top={false} bottom={false} left={false}") &&
      sidebarFrame.includes("$bindable(false)") &&
      !sidebarFrame.includes("BevelLine") &&
      sidebarCss.includes(".sidebar-frame.folded") &&
      sidebarCss.includes("display: none") &&
      sidebarCss.includes("display: grid"),
    "Desktop shell boundaries must use one Bevel edge and Mobile must share Header fold state",
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
    "The design language and showroom must align same-layer lines and inset nested layers as a whole",
  );
  assert(
    !sidebarCss.includes("border-right:") &&
      !sidebarCss.includes("border-bottom:") &&
      !showroomCss.includes("border-top: 1px solid") &&
      !showroomCss.includes("border-bottom: 1px solid"),
    "Shell and showroom structural boundaries must not fall back to one-sided solid borders",
  );
});
