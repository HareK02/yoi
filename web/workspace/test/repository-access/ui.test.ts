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
      loaderSource.includes("parseRepositorySshPublicKey") &&
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

test("Repository Access generates and copies selectable public keys", () => {
  for (
    const token of [
      "/credentials/generate",
      "/public-key",
      "Generate Repository SSH credential",
      "navigator.clipboard.writeText",
      "publicKeys[credential.credential_id]",
      "workspace-default",
      "always offered during SSH clone",
    ]
  ) {
    assert(
      source.includes(token),
      `missing generated public key flow ${token}`,
    );
  }
  assert(
    source.includes("binding.credential_id"),
    "Repository bindings should identify the selected credential",
  );
});

test("Repository Access hides Rotate for the Workspace default credential", () => {
  const credentialsStart = source.indexOf("<h3>SSH credentials</h3>");
  const generateStart = source.indexOf(
    "<h3>Generate Repository SSH credential</h3>",
  );
  assert(
    credentialsStart >= 0 && generateStart > credentialsStart,
    "credential list should appear before the generation form",
  );

  const credentialList = source.slice(credentialsStart, generateStart);
  const additionalCredentialGuard = credentialList.indexOf(
    "{#if credential.credential_id !== workspaceDefaultCredentialId}",
  );
  const rotateAction = credentialList.indexOf(
    "rotateCredentialId = rotateCredentialId === credential.credential_id",
    additionalCredentialGuard,
  );
  const deleteAction = credentialList.indexOf(
    "onclick={() => void deleteCredential(credential)}",
    rotateAction,
  );
  const guardEnd = credentialList.indexOf("{/if}", deleteAction);

  assert(
    additionalCredentialGuard >= 0 &&
      rotateAction > additionalCredentialGuard &&
      deleteAction > rotateAction &&
      guardEnd > deleteAction,
    "Rotate and Delete should render only for additional credentials",
  );
  assert(
    credentialList.indexOf(
      "{#if rotateCredentialId === credential.credential_id}",
      guardEnd,
    ) > guardEnd,
    "the existing rotation form should remain available after selecting an additional credential",
  );
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
