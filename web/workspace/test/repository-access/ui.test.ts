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
