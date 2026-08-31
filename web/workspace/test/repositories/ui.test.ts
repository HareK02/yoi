type TestRegistrar = (name: string, fn: () => void | Promise<void>) => void;

const test =
  (globalThis as unknown as { Deno: { test: TestRegistrar } }).Deno.test;

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const pageSource = await Deno.readTextFile(
  new URL(
    "../../src/routes/w/[workspaceId]/settings/repositories/+page.svelte",
    import.meta.url,
  ),
);
const loadSource = await Deno.readTextFile(
  new URL(
    "../../src/routes/w/[workspaceId]/settings/repositories/+page.ts",
    import.meta.url,
  ),
);

test("Repository settings use the scoped list and typed create collection", () => {
  for (const token of [
    "workspaceApiPath(params.workspaceId, \"/repositories\")",
    "workspaceApiPath(data.workspaceId, '/repositories')",
    "method: 'POST'",
    "repository_id: repositoryId",
    "display_name: displayName",
    "default_ref: defaultRef || null",
    "await invalidateAll()",
  ]) {
    assert(
      pageSource.includes(token) || loadSource.includes(token),
      `Repository settings should include ${token}`,
    );
  }
});

test("Repository Add form keeps access secrets outside registration input", () => {
  for (const forbidden of [
    "private_key",
    "passphrase",
    "credential_id",
    "host_trust_id",
  ]) {
    assert(
      !pageSource.includes(forbidden),
      `Repository registration must not accept ${forbidden}`,
    );
  }
  assert(
    pageSource.includes("/settings/repository-access"),
    "remote Repository rows should link to Repository Access",
  );
});
