import { assertRejects } from "@std/assert";
import { join } from "@std/path";
import {
  authMetadataPath,
  deleteAuthState,
  validateAuthState,
  writeAuthMetadata,
} from "../src/auth_state.ts";

Deno.test("auth state is bound to persona, base origin, and expiry", async () => {
  const directory = await Deno.makeTempDir();
  try {
    const state = join(directory, "owner.json");
    await Deno.writeTextFile(state, '{"cookies":[],"origins":[]}');
    await writeAuthMetadata(state, "owner", "https://example.test/path", 1);
    await validateAuthState(state, "owner", "https://example.test/other");
    await assertRejects(
      () => validateAuthState(state, "non-owner", "https://example.test"),
      Error,
      "does not match persona",
    );
    await assertRejects(
      () => validateAuthState(state, "owner", "https://other.test"),
      Error,
      "belongs to https://example.test",
    );
    await assertRejects(
      () =>
        validateAuthState(
          state,
          "owner",
          "https://example.test",
          new Date(Date.now() + 2 * 60 * 60 * 1000),
        ),
      Error,
      "auth state expired",
    );
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});

Deno.test("auth state deletion removes state and metadata idempotently", async () => {
  const directory = await Deno.makeTempDir();
  try {
    const state = join(directory, "owner.json");
    await Deno.writeTextFile(state, "{}");
    await writeAuthMetadata(state, "owner", "http://127.0.0.1:3000", 1);
    await deleteAuthState(state);
    await deleteAuthState(state);
    await assertRejects(() => Deno.stat(state), Deno.errors.NotFound);
    await assertRejects(() => Deno.stat(authMetadataPath(state)), Deno.errors.NotFound);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
});
