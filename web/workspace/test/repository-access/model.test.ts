import {
  repositoryAccessRequestError,
  validateCredentialRotation,
  validateGeneratedCredential,
  validateHostTrust,
  validateImportedCredential,
} from "../../src/lib/workspace/repository-access/model.ts";

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("Repository Access validates disclosed credential forms before mutation", () => {
  const generated = validateGeneratedCredential({
    credentialId: "invalid id",
    name: "",
  });
  assert(
    generated.credentialId?.includes("letters"),
    "invalid credential id should have a field error",
  );
  assert(
    generated.name === "Name is required.",
    "missing name should have a field error",
  );

  const imported = validateImportedCredential({
    credentialId: "deploy-key",
    name: "Deploy key",
    privateKey: "",
  });
  assert(
    imported.privateKey?.includes("required"),
    "missing private key should have a field error",
  );
  assert(
    Object.keys(validateCredentialRotation("  ")).length === 1,
    "rotation should reject an empty private key",
  );
});

Deno.test("Repository Access validates host identity fields in the editor scope", () => {
  const errors = validateHostTrust({
    hostTrustId: "host trust",
    hostname: "",
    port: 70_000,
    hostKey: "",
  });
  for (const field of ["hostTrustId", "hostname", "port", "hostKey"]) {
    assert(
      Boolean(errors[field]),
      `missing host trust field error for ${field}`,
    );
  }
  assert(
    Object.keys(validateHostTrust({
      hostTrustId: "github-com",
      hostname: "github.com",
      port: 22,
      hostKey: "ssh-ed25519 fixture-public-host-key",
    })).length === 0,
    "valid host trust fields should pass",
  );
});

Deno.test("Repository Access distinguishes permission, conflict, field, and missing-item failures", () => {
  assert(
    repositoryAccessRequestError(403, "rotate the key").message.includes(
      "permission",
    ),
    "403 should explain permission",
  );
  assert(
    repositoryAccessRequestError(409, "rotate the key").message.includes(
      "Reload",
    ),
    "409 should explain revision recovery",
  );
  assert(
    repositoryAccessRequestError(422, "rotate the key").message.includes(
      "fields",
    ),
    "422 should point to fields",
  );
  assert(
    repositoryAccessRequestError(404, "rotate the key").message.includes(
      "no longer exists",
    ),
    "404 should explain stale target",
  );
});
