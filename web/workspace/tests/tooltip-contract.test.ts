// @ts-nocheck
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Tooltip exposes the same contextual help to pointer and keyboard users", async () => {
  const source = await Deno.readTextFile(
    new URL("../src/lib/workspace/ui/Tooltip.svelte", import.meta.url),
  );

  for (
    const contract of [
      'role="tooltip"',
      "onpointerenter={showAfterDelay}",
      "onfocusin={showImmediately}",
      "event.key === 'Escape'",
      "children: Snippet<[descriptionId: string]>",
      "max-width: min(20rem, calc(100vw - var(--space-4)))",
    ]
  ) {
    assert(source.includes(contract), `Tooltip must preserve ${contract}`);
  }
});

Deno.test("Tooltip showroom binds help to actions and column headings", async () => {
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

  for (const descriptionId of ["rerun-help", "last-seen-help", "source-help"]) {
    assert(
      workspaceSource.includes(descriptionId),
      `Workspace showroom must expose contextual help ${descriptionId}`,
    );
  }
  assert(
    settingsSource.includes("retry-runtime-help"),
    "Settings showroom must expose retry operation help",
  );
  assert(
    workspaceSource.includes('class="column-help"') &&
      workspaceSource.includes("aria-describedby={descriptionId}>Last seen"),
    "Column help must be bound to the column label rather than a separate info button",
  );
  assert(
    !workspaceSource.includes("Actions explain") &&
      !workspaceSource.includes("Use a table when"),
    "Showroom must not restore persistent operation or table explanation",
  );
});
