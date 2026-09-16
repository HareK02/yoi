// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Bevel keeps depth visual while semantic controls remain children", async () => {
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
      "export type BevelProfile = 'edge' | 'ridge'",
      "export type BevelDepth = 'raised' | 'inset'",
      "children: Snippet<[]>",
      "top?: boolean",
      "right?: boolean",
      "bottom?: boolean",
      "left?: boolean",
      "pointer-events: none",
      "data-pressed={pressed || undefined}",
      "aria-invalid='true'",
      "forced-colors: active",
      "mask-composite: exclude",
    ]
  ) {
    assert(source.includes(contract), `Bevel must preserve ${contract}`);
  }

  assert(
    source.includes("@supports") &&
      source.includes("border-style: solid") &&
      source.includes(
        "border-width: var(--_top-w) var(--_right-w) var(--_bottom-w) var(--_left-w)",
      ),
    "Bevel must retain a side-selective plain border fallback around the enhanced edge",
  );
  assert(
    source.includes("calc(max(cos(180deg - var(--_L)), 0) * 100%)") &&
      source.includes("var(--_s6) 90deg") &&
      source.includes("var(--_s6) 270deg") &&
      !source.includes("+ 1) * 50%"),
    "Bevel must clamp unlit faces to the configured shadow without ambient light",
  );
  assert(
    appCss.includes("--bevel-face-width: 2px") &&
      source.includes("--_W: var(--bevel-face-width)") &&
      source.includes("inset: var(--bevel-face-width)") &&
      !source.includes("BevelSize") &&
      !source.includes("data-size"),
    "Bevel must use the project-wide 2px edge and compose ridge from two 2px faces",
  );
  assert(
    source.includes("data-top={top ? 'true' : 'false'}") &&
      source.includes("data-right={right ? 'true' : 'false'}") &&
      source.includes("data-bottom={bottom ? 'true' : 'false'}") &&
      source.includes("data-left={left ? 'true' : 'false'}") &&
      source.includes(".bevel[data-top='false'] {") &&
      source.includes("--bevel-top-left-radius: 0px") &&
      source.includes("--bevel-top-right-radius: 0px") &&
      source.includes(".bevel[data-right='false'] {") &&
      source.includes("--bevel-bottom-right-radius: 0px") &&
      source.includes(".bevel[data-bottom='false'] {") &&
      source.includes("--bevel-bottom-left-radius: 0px") &&
      source.includes(".bevel[data-left='false'] {") &&
      source.includes("--bevel-top-inset: 0px") &&
      source.includes("--bevel-right-inset: 0px") &&
      source.includes("--bevel-bottom-inset: 0px") &&
      source.includes("--bevel-left-inset: 0px"),
    "Bevel must draw selected sides and round only corners shared by two enabled sides",
  );
  assert(
    source.includes("data-profile={profile}") &&
      source.includes("[data-profile='ridge'][data-depth='raised']::after") &&
      source.includes("[data-profile='ridge'][data-depth='inset']::after") &&
      !source.includes("data-depth='ridge'"),
    "Bevel must keep edge/ridge profile orthogonal to raised/inset depth",
  );
  assert(
    appCss.includes("--bevel-highlight: #fff") &&
      appCss.includes("--bevel-shadow: #000") &&
      !source.includes("surface") &&
      !source.includes("tone") &&
      !source.includes("background-color:"),
    "Bevel must use the shared highlight and shadow palette without owning a surface tone",
  );
  assert(
    designLanguage.includes("Bevelはedge") &&
      designLanguage.includes("lightingだけを所有する") &&
      designLanguage.includes("text/content areaだけとする"),
    "The design language must prohibit Bevel surface colors outside the child content area",
  );
  assert(
    source.includes("button, input, select, textarea"),
    "Bevel must leave native semantic controls inside the visual wrapper",
  );
});

Deno.test("Workspace showroom exercises raised inset and ridge edges", async () => {
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

  assert(
    workspaceSource.includes('profile="edge" depth="raised"') &&
      workspaceSource.includes('profile="ridge" depth="raised"') &&
      workspaceSource.includes('profile="ridge" depth="inset"') &&
      workspaceSource.includes("right={false} bottom={false}") &&
      workspaceSource.includes("top={false} left={false}") &&
      workspaceSource.includes("pressed"),
    "Workspace showroom must demonstrate edge and ridge depths with full and adjacent side selections",
  );
  assert(
    settingsSource.includes('profile="edge" depth="inset"') &&
      settingsSource.includes('aria-invalid="true"'),
    "Settings showroom must demonstrate inset and invalid fields",
  );
});
