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
    appCss.includes("--bevel-face-width: 2px") &&
      source.includes("--bevel-line-face-width: var(--bevel-face-width)") &&
      source.includes(
        "--bevel-line-width: calc(var(--bevel-line-face-width) * 2)",
      ) &&
      !source.includes("BevelLineWeight") &&
      !source.includes("data-weight"),
    "BevelLine must compose a fixed 4px ridge from two project-wide 2px faces",
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

Deno.test("Workspace shell and showroom use BevelLine for structural separators", async () => {
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

  assert(
    rootLayout.includes("app-shell__topbar-divider") &&
      sidebarFrame.includes("sidebar-frame__divider--vertical") &&
      sidebarFrame.includes("sidebar-frame__divider--horizontal"),
    "Workspace shell must render structural dividers through BevelLine",
  );
  assert(
    !sidebarCss.includes("border-right:") &&
      !sidebarCss.includes("border-bottom:") &&
      !showroomCss.includes("border-top: 1px solid") &&
      !showroomCss.includes("border-bottom: 1px solid"),
    "Shell and showroom structural separators must not fall back to one-sided solid borders",
  );
});
