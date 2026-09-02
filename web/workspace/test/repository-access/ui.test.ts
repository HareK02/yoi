type TestRegistrar = (name: string, body: () => void | Promise<void>) => void;

const test =
  (globalThis as unknown as { Deno: { test: TestRegistrar } }).Deno.test;

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const source = await Deno.readTextFile(
  new URL(
    "../../src/routes/w/[workspaceId]/settings/repository-access/+page.svelte",
    import.meta.url,
  ),
);
const loaderSource = await Deno.readTextFile(
  new URL(
    "../../src/routes/w/[workspaceId]/settings/repository-access/+page.ts",
    import.meta.url,
  ),
);

test("Repository Access Web code consumes workspace-api generated DTOs", () => {
  assert(
    source.includes("$lib/generated/repository-access-api"),
    "mutation code should import generated request and response contracts",
  );
  assert(
    loaderSource.includes("parseRepositorySshCredentials") &&
      loaderSource.includes("parseRepositorySshHostTrusts") &&
      loaderSource.includes("parseRepositoryAccessProjection"),
    "loader should validate unknown JSON before exposing generated DTOs to Svelte",
  );
  assert(
    loaderSource.indexOf('"/settings/repository-access"') <
      loaderSource.indexOf("Promise.all"),
    "loader should check Repository Access permission before starting list preloads",
  );
  for (
    const duplicate of [
      "interface RepositorySshCredential",
      "interface RepositorySshHostTrust",
      "interface RepositoryAccessProjection",
    ]
  ) {
    assert(
      !loaderSource.includes(duplicate) && !source.includes(duplicate),
      `Web code must not redeclare ${duplicate}`,
    );
  }
});

test("Repository Access renders the shared access projection fields", () => {
  for (
    const field of [
      "accessProjection.config_revision",
      "accessProjection.projection_digest",
      "accessProjection.bindings",
      "binding.repository_key",
      "binding.credential_id",
      "binding.host_trust_id",
      "binding.access",
    ]
  ) {
    assert(source.includes(field), `missing access projection field ${field}`);
  }
});

test("Repository credential submissions clear write-only fields in finally blocks", () => {
  const createStart = source.indexOf("async function createCredential()");
  const rotateStart = source.indexOf("async function rotateCredential(");
  const deleteStart = source.indexOf("async function deleteCredential(");
  assert(
    createStart >= 0 && rotateStart > createStart && deleteStart > rotateStart,
    "credential handlers should appear in source order",
  );

  const createBody = source.slice(createStart, rotateStart);
  const rotateBody = source.slice(rotateStart, deleteStart);
  for (const token of ["finally", "privateKey = ''", "passphrase = ''"]) {
    assert(
      createBody.includes(token),
      `create handler should contain ${token}`,
    );
  }
  for (
    const token of [
      "finally",
      "rotatePrivateKey = ''",
      "rotatePassphrase = ''",
    ]
  ) {
    assert(
      rotateBody.includes(token),
      `rotate handler should contain ${token}`,
    );
  }
});
