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
    listLoader.includes('"/settings/signing-identity"') &&
      !listLoader.includes("/settings/workspace"),
    "Runtime list loader should use the canonical Workspace signing identity route",
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

Deno.test("Runtime registration presents the complete multi-Workspace trust sequence", async () => {
  const page = await Deno.readTextFile(
    new URL(
      "../src/routes/w/[workspaceId]/settings/runtimes/+page.svelte",
      import.meta.url,
    ),
  );

  for (
    const token of [
      "1. Trust this Workspace on the Runtime",
      "Provision Workspace identity",
      "provisionWorkspaceSigningIdentity(data.workspaceId)",
      "Existing Workspace trust entries are preserved.",
      "--fs-root",
      "--fs-runtime-dir",
      "2. Verify the Runtime identity",
      "yoi-runtime identity show --json",
      "3. Register the connection",
      "Register Runtime",
      "Run Test to complete authenticated verification.",
    ]
  ) {
    assert(
      page.includes(token),
      `Runtime registration should include ${token}`,
    );
  }
  assert(
    page.includes(
      "data.signingIdentity?.identity.state === 'pending_provisioning'",
    ) &&
      page.includes("Loading Workspace public identity…"),
    "pending identity must have a dedicated provisioning state before loading fallback",
  );
  assert(
    !page.includes("fingerprintConfirmation") &&
      !page.includes("Confirm Runtime fingerprint"),
    "Runtime registration must not require retyping a fingerprint",
  );
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
      "Revoke Workspace trust",
      "Workspace trust only; this does not delete the Runtime process, Workers, or Workdirs.",
      "await revokeRuntimeTrustKey(",
      "deleteRemoteRuntime(data.workspaceId, operation.runtimeId)",
      "Revoke trust and delete registration",
      "trust.status !== 'revoked'",
      "deleteRuntimeConfirmation.trim() !== data.runtimeId",
      "Delete Runtime registration",
      "Delete registration",
      "This does not stop the Runtime process",
      "RuntimeTrustConflictError",
      "RuntimeTrustRouteFence",
      "routeFence.enter(data.runtimeId)",
      "showPublicKey = false",
      "revealedPublicKey = null",
      "publicKey = ''",
      "requestError = null",
      "successMessage = null",
      "isCurrentRoute(operation)",
      "revealRuntimeTrustKey",
      "await reloadAuthority()",
      "busyAction !== null",
      "Workdirs",
      "Recent trust audit",
    ]
  ) {
    assert(page.includes(token), `Runtime detail should include ${token}`);
  }
  for (
    const token of [
      "fingerprintConfirmation",
      "revokeFingerprintConfirmation",
      "Confirm current fingerprint",
    ]
  ) {
    assert(!page.includes(token), `Runtime detail must not require ${token}`);
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
