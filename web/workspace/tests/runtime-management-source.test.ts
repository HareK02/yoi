declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
  readTextFile(path: URL): Promise<string>;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Runtime Settings routes validate unknown JSON through the shared Runtime parser", async () => {
  const [listLoader, detailLoader] = await Promise.all([
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/runtimes/+page.ts",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/runtimes/[runtimeId]/+page.ts",
        import.meta.url,
      ),
    ),
  ]);

  assert(
    listLoader.includes("parseWorkspaceRuntimeList(value)"),
    "Runtime list loader should validate unknown JSON",
  );
  assert(
    detailLoader.includes("parseWorkspaceRuntimeDetail(value)"),
    "Runtime detail loader should validate unknown JSON",
  );
  for (const source of [listLoader, detailLoader]) {
    assert(
      !source.includes("loadJson<"),
      "Runtime loaders must not cast response JSON to a handwritten DTO",
    );
  }
});

Deno.test("Runtime list links to canonical detail and has no inline delete action", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/settings/runtimes/+page.svelte",
      import.meta.url,
    ),
  );

  assert(
    page.includes(
      "/settings/runtimes/${encodeURIComponent(runtime.runtime_id)}",
    ),
    "Runtime name should link to canonical detail",
  );
  assert(
    page.includes("testRuntime(runtime)"),
    "connection Test should remain available",
  );
  assert(page.includes("Add Runtime"), "Add Runtime should remain available");
  assert(
    page.includes("data.workspace.permissions.manage_runtimes"),
    "Add Runtime should be hidden from non-owners",
  );
  assert(
    !page.includes("deleteRuntime"),
    "inline Runtime delete logic must be removed",
  );
  assert(
    !page.includes(">Delete</button>"),
    "inline Runtime delete control must be removed",
  );
});

Deno.test("Runtime detail keeps trust controls owner-only and conflict-safe", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/settings/runtimes/[runtimeId]/+page.svelte",
      import.meta.url,
    ),
  );

  const ownerGate = page.indexOf("data.workspace.permissions.manage_runtimes");
  const reveal = page.indexOf("Reveal public key");
  const mutation = page.indexOf('id="runtime-public-key-input"');
  assert(ownerGate >= 0, "Runtime trust controls should use manage_runtimes");
  assert(
    page.includes("Current fingerprint"),
    "current fingerprint must be explicit",
  );
  assert(
    page.includes("Replacement fingerprint"),
    "replacement fingerprint must be previewed before confirmation",
  );
  assert(
    page.includes("!runtime.management.built_in"),
    "Runtime trust controls should be hidden for the built-in Runtime",
  );
  assert(
    ownerGate < reveal && ownerGate < mutation,
    "owner gate should wrap key controls",
  );

  for (
    const token of [
      "Create Workspace trust",
      "Replace trusted key",
      "Reactivate with this key",
      "Confirm current fingerprint",
      "Revoke Workspace trust",
      "Workspace trust only; this does not delete the Runtime process, Workers, or Workdirs.",
      "RuntimeTrustConflictError",
      "await reloadAuthority()",
      "busyAction !== null",
      "Workdirs",
      "Recent trust audit",
    ]
  ) {
    assert(page.includes(token), `Runtime detail should include ${token}`);
  }
});

Deno.test("Runtime detail uses flat sections instead of nested cards", async () => {
  const [page, css] = await Promise.all([
    Deno.readTextFile(
      new URL(
        "../src/routes/w/[workspaceId]/settings/runtimes/[runtimeId]/+page.svelte",
        import.meta.url,
      ),
    ),
    Deno.readTextFile(
      new URL("../src/lib/workspace/styles/settings.css", import.meta.url),
    ),
  ]);

  assert(
    !page.includes('class="card"') && !page.includes("settings-card"),
    "Runtime detail should not add card nesting",
  );
  assert(
    css.includes(".runtime-detail-section") &&
      css.includes("border-top: 1px solid var(--line)"),
    "Runtime detail hierarchy should use flat section separators",
  );
});
