declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals(actual: unknown, expected: unknown): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertThrows(fn: () => unknown): void {
  try {
    fn();
  } catch {
    return;
  }
  throw new Error("Expected rejection");
}

async function assertRejects(
  fn: () => Promise<unknown>,
  message: string,
): Promise<Error> {
  try {
    await fn();
  } catch (error) {
    if (error instanceof Error && error.message.includes(message)) return error;
    throw new Error("Unexpected rejection");
  }
  throw new Error("Expected rejection");
}
import {
  HostingRequestError,
  loadHostingAccess,
  loadHostingSettings,
  loadRegistrationPolicy,
  parseHostingAccess,
  parseHostingSettings,
  saveHostingSettings,
} from "./hosting.ts";

Deno.test("hosting projections accept only an exact boolean field", () => {
  for (const enabled of [true, false]) {
    assertEquals(parseHostingSettings({ self_registration_enabled: enabled }), {
      self_registration_enabled: enabled,
    });
    assertEquals(parseHostingAccess({ is_admin: enabled }), {
      is_admin: enabled,
    });
  }
  for (
    const value of [
      null,
      [],
      {},
      true,
      { self_registration_enabled: "true" },
      { self_registration_enabled: 1 },
      { self_registration_enabled: true, secret: "unexpected" },
      { is_admin: true },
    ]
  ) {
    assertThrows(() => parseHostingSettings(value));
  }
  for (
    const value of [null, [], {}, { is_admin: "true" }, {
      is_admin: true,
      user: "admin",
    }, { self_registration_enabled: true }]
  ) {
    assertThrows(() => parseHostingAccess(value));
  }
});

Deno.test("hosting adapters use distinct public/access/admin endpoints and same-origin credentials", async () => {
  const original = globalThis.fetch;
  const requests: { path: string; init?: RequestInit }[] = [];
  globalThis.fetch = ((input, init) => {
    const path = String(input);
    requests.push({ path, init });
    return Promise.resolve(
      Response.json(
        path.endsWith("/access")
          ? { is_admin: false }
          : { self_registration_enabled: false },
      ),
    );
  }) as typeof fetch;
  try {
    assertEquals(await loadRegistrationPolicy(), {
      self_registration_enabled: false,
    });
    assertEquals(await loadHostingAccess(), { is_admin: false });
    assertEquals(await loadHostingSettings(), {
      self_registration_enabled: false,
    });
    assertEquals(
      await saveHostingSettings({ self_registration_enabled: false }),
      { self_registration_enabled: false },
    );
    assertEquals(requests.map((item) => item.path), [
      "/api/auth/registration-policy",
      "/api/hosting/access",
      "/api/hosting/settings",
      "/api/hosting/settings",
    ]);
    assertEquals(
      requests.map((item) => item.init?.credentials),
      Array(4).fill("same-origin"),
    );
    assertEquals(
      requests.map((item) => item.init?.cache),
      Array(4).fill("no-store"),
    );
    assertEquals(requests[3].init?.method, "PUT");
    assertEquals(JSON.parse(String(requests[3].init?.body)), {
      self_registration_enabled: false,
    });
  } finally {
    globalThis.fetch = original;
  }
});

Deno.test("hosting failures expose status without response bodies and reject malformed success", async () => {
  const original = globalThis.fetch;
  try {
    for (const status of [401, 403, 503]) {
      globalThis.fetch = () =>
        Promise.resolve(new Response("private diagnostic", { status }));
      const error = await assertRejects(
        loadHostingSettings,
        `Hosting request failed (${status}).`,
      );
      if (!(error instanceof HostingRequestError)) {
        throw new Error("Expected HTTP status error");
      }
      assertEquals(error.status, status);
    }
    globalThis.fetch = () =>
      Promise.resolve(Response.json({ is_admin: "true" }));
    await assertRejects(loadHostingAccess, "Invalid hosting response");
    globalThis.fetch = () => Promise.resolve(new Response("invalid JSON"));
    await assertRejects(loadRegistrationPolicy, "not valid JSON");
  } finally {
    globalThis.fetch = original;
  }
});
