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
const settingsCss = await Deno.readTextFile(
  new URL("../../src/lib/workspace/styles/settings.css", import.meta.url),
);

test("Repository Access consumes generated contracts and validates every response boundary", () => {
  assert(
    source.includes("$lib/generated/repository-access-api"),
    "mutation code should import generated request and response contracts",
  );
  for (
    const parser of [
      "parseRepositorySshCredentials",
      "parseRepositorySshHostTrusts",
      "parseRepositorySshPublicKey",
      "parseRepositoryAccessProjection",
    ]
  ) {
    assert(loaderSource.includes(parser), `loader should use ${parser}`);
  }
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

test("Repository Access authorizes first and preserves independently available sections", () => {
  assert(
    loaderSource.indexOf('"/settings/repository-access"') <
      loaderSource.indexOf("Promise.all"),
    "loader should check Repository Access permission before section preloads",
  );
  assert(
    loaderSource.match(/loadRepositoryAccessSection/g)?.length === 4,
    "credentials, host trusts, and the default public key should have independent failures",
  );
  for (
    const field of [
      "credentialsError",
      "hostTrustsError",
      "defaultPublicKeyError",
      "accessProjectionError",
    ]
  ) {
    assert(
      source.includes(`data.${field}`),
      `page should render scoped ${field}`,
    );
  }
  assert(
    loaderSource.includes("WORKSPACE_DEFAULT_CREDENTIAL_ID") &&
      !loaderSource.includes("credentials.map("),
    "loader should preload only the default public key instead of every public key",
  );
});

test("Repository Access presents current state as tables with technical disclosure", () => {
  for (
    const token of [
      "Repository bindings",
      "SSH credentials",
      "Pinned SSH host keys",
      "repository-access-table-wrap",
      "Technical details",
      "Credential revisions",
      "Host key revisions",
      "accessProjection.config_revision",
      "accessProjection.projection_digest",
      "binding.repository_key",
      "binding.credential_id",
      "binding.host_trust_id",
      "binding.access",
    ]
  ) {
    assert(
      source.includes(token),
      `missing information structure token ${token}`,
    );
  }
  assert(
    !source.includes('class="card'),
    "Repository Access should not nest generic cards",
  );
  assert(
    !source.includes("<h2>Repository Access</h2>"),
    "the route title should remain owned by the Header",
  );
  assert(
    !source.includes("<textarea readonly"),
    "public keys should not occupy permanent textareas",
  );
  assert(
    settingsCss.includes(".repository-access-table-wrap") &&
      settingsCss.includes("overflow-x: auto") &&
      settingsCss.includes(".repository-access-form-grid"),
    "Settings CSS should own bounded table overflow and responsive forms",
  );
});

test("Repository Access explains automatic application without inventing a binding", () => {
  for (
    const token of [
      "No explicit bindings",
      "Workspace default credential",
      "unique pinned key that matches their host and port",
      "Explicit binding",
      "plus <code>{binding.credential_id}",
    ]
  ) {
    assert(source.includes(token), `missing access authority copy ${token}`);
  }
  assert(
    source.includes("A successful probe only observes a key") &&
      source.includes("Git authentication succeeds only"),
    "probe, trust persistence, and Git authentication must remain distinct",
  );
});

test("Repository Access keeps mutation forms disclosed and operation state scoped", () => {
  for (
    const token of [
      "credentialForm === 'generate'",
      "credentialForm === 'import'",
      "hostEditorOpen",
      "selectedCredential",
      "validateGeneratedCredential",
      "validateImportedCredential",
      "validateCredentialRotation",
      "validateHostTrust",
      'class="field-error"',
      "aria-invalid",
      "pendingOperations",
      "credentialFormNotice",
      "credentialRowNotices",
      "hostFormNotice",
      "hostRowNotices",
      ">Cancel</button>",
    ]
  ) {
    assert(source.includes(token), `missing scoped form structure ${token}`);
  }
  assert(
    !source.includes("disabled={busy}") && !source.includes("activeOperation"),
    "unrelated controls must not share page-wide mutation state",
  );
  const permanentFormStart = source.indexOf("<form");
  const generateDisclosure = source.indexOf(
    "{#if credentialForm === 'generate'}",
  );
  assert(
    permanentFormStart > generateDisclosure,
    "forms should render only after an explicit disclosure action",
  );
});

test("Repository Access keeps public key copy easy without exposing key material", () => {
  for (
    const token of [
      "/public-key",
      "navigator.clipboard.writeText",
      "Copy public key",
      "workspace-default",
      "publicKeys[credential.credential_id]",
      "data-web-ux-redact",
    ]
  ) {
    assert(
      source.includes(token),
      `missing safe public key or secret handling ${token}`,
    );
  }
  assert(
    source.includes("publicKeys = { ...publicKeys") &&
      source.includes("loadPublicKey(credential.credential_id)"),
    "non-default public keys should load only on demand",
  );
});

test("Repository Access protects default and referenced resources from destructive controls", () => {
  const defaultGuard = source.indexOf(
    "{#if credential.credential_id !== workspaceDefaultCredentialId}",
  );
  const rotateAction = source.indexOf(
    "openCredentialRotation(credential)",
    defaultGuard,
  );
  const unreferencedGuard = source.indexOf(
    "{#if credential.referenced_repositories.length === 0}",
    rotateAction,
  );
  const deleteAction = source.indexOf(
    "deleteCredential(credential)",
    unreferencedGuard,
  );
  assert(
    defaultGuard >= 0 && rotateAction > defaultGuard &&
      unreferencedGuard > rotateAction && deleteAction > unreferencedGuard,
    "default and referenced credentials should not render destructive actions",
  );
  assert(
    source.includes("Remove Repository references before deletion") &&
      source.includes("hostTrust.referenced_repositories.length === 0"),
    "reference constraints should be visible beside affected rows",
  );
});

test("Repository credential submissions clear write-only fields after attempts", () => {
  const createStart = source.indexOf("async function createCredential()");
  const rotateStart = source.indexOf("async function rotateCredential(");
  const deleteStart = source.indexOf("async function deleteCredential(");
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
