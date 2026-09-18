// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function collectStyleSources(directory: URL): Promise<URL[]> {
  const files: URL[] = [];

  for await (const entry of Deno.readDir(directory)) {
    const url = new URL(entry.name, directory);
    if (entry.isDirectory) {
      url.pathname += "/";
      files.push(...await collectStyleSources(url));
    } else if (
      entry.isFile &&
      (entry.name.endsWith(".css") || entry.name.endsWith(".svelte"))
    ) {
      files.push(url);
    }
  }

  return files;
}

Deno.test("Workspace typography uses only the title body and compact scales", async () => {
  const sourceRoot = new URL("../src/", import.meta.url);
  const allowedSizes = new Set([
    "var(--font-size-title)",
    "var(--font-size-body)",
    "var(--font-size-compact)",
    "inherit",
  ]);

  for (const url of await collectStyleSources(sourceRoot)) {
    const source = await Deno.readTextFile(url);
    for (const match of source.matchAll(/font-size:\s*([^;]+);/g)) {
      const value = match[1].trim();
      assert(
        allowedSizes.has(value),
        `${url.pathname} must not use the font-size value ${value}`,
      );
    }
    assert(
      !/font:\s*[^;]*(?:\d+(?:\.\d+)?(?:px|rem|em))[^;]*;/g.test(source),
      `${url.pathname} font shorthand must use a shared typography token`,
    );
  }
});

Deno.test("Workspace typography tokens bind the documented three-level scale", async () => {
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
      "--font-size-title: 24px",
      "--line-height-title: 32px",
      "--font-size-body: 14px",
      "--line-height-body: 20px",
      "--font-size-compact: 12px",
      "--line-height-compact: 16px",
      "font-size: var(--font-size-title)",
      "font-size: var(--font-size-body)",
      "font-size: var(--font-size-compact)",
    ]
  ) {
    assert(appCss.includes(contract), `app.css must preserve ${contract}`);
  }

  assert(
    designLanguage.includes("--font-size-title /* 24px */") &&
      designLanguage.includes("--font-size-body /* 14px */") &&
      designLanguage.includes("--font-size-compact /* 12px */") &&
      designLanguage.includes("`12px`未満と`13px`を使わない"),
    "The design language must preserve the title, body, and compact typography contract",
  );
});
