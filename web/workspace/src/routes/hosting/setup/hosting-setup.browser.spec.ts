// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import Setup from "./+page.svelte";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "#lib/workspace/auth/api.test-fixtures.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// UI→本番adapter/serializerを接続。fetchとbrowser credentialだけ代替する。
// Backend認証・権限の拒否や署名を検証する試験ではない。
function boundary(
  { status = 200, admin = true, actor = true, failLogin = false } = {},
) {
  vi.stubGlobal("isSecureContext", true);
  const create = vi.fn(async () => credentialFixture());
  const get = vi.fn(async () => credentialFixture(true));
  vi.stubGlobal("navigator", { credentials: { create, get } });
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") {
      return Response.json({
        session: actor
          ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
          : { kind: "anonymous" },
        actor: actor ? { ...authUser, auth_method: "browser_session" } : null,
      });
    }
    if (path === "/api/hosting/access") {
      return Response.json({ is_admin: admin });
    }
    if (path === "/api/auth/passkeys/addition/options") {
      return status === 200
        ? Response.json(registrationOptions)
        : new Response("host-test-token", { status });
    }
    if (path === "/api/auth/passkeys/addition/complete") {
      return Response.json({ user: authUser });
    }
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path === "/api/auth/passkeys/login/complete") {
      return failLogin
        ? new Response(null, { status: 401 })
        : Response.json({ user: authUser });
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  vi.stubGlobal("fetch", fetcher);
  return { fetcher, create, get };
}
async function enroll() {
  await fireEvent.input(screen.getByLabelText("Host setup token"), {
    target: { value: "host-test-token" },
  });
  await fireEvent.click(
    screen.getByRole("button", { name: "Add administrator passkey" }),
  );
}

test("public shell does not request or grant anything without explicit host token input", () => {
  const { fetcher, create } = boundary();
  render(Setup);
  const input = screen.getByLabelText("Host setup token") as HTMLInputElement;
  expect(input.type).toBe("password");
  expect(input.autocomplete).toBe("off");
  expect(input.getAttribute("name")).toBeNull();
  expect(
    (screen.getByRole("button", {
      name: "Add administrator passkey",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
  expect(fetcher).not.toHaveBeenCalled();
  expect(create).not.toHaveBeenCalled();
});

test("enrollment clears token and omits cookies, then verifies an actual passkey login before settings", async () => {
  const { fetcher, create, get } = boundary();
  const localWrite = vi.spyOn(window.localStorage, "setItem");
  const sessionWrite = vi.spyOn(window.sessionStorage, "setItem");
  render(Setup);
  await enroll();
  await screen.findByRole("button", { name: "Sign in with new passkey" });
  expect(fetcher.mock.calls.some(([path]) => path === "/api/hosting/access"))
    .toBe(false);
  expect(create).toHaveBeenCalledTimes(1);
  expect(get).not.toHaveBeenCalled();
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
  expect(document.body.textContent).not.toContain("host-test-token");
  expect(localWrite).not.toHaveBeenCalled();
  expect(sessionWrite).not.toHaveBeenCalled();
  const requests = fetcher.mock.calls;
  for (const [path, init] of requests) {
    expect(String(path)).not.toContain("host-test-token");
    expect(String(init?.body)).not.toContain("host-test-token");
    expect(new Headers(init?.headers).get("authorization")).toBe(
      String(path).includes("/addition/") ? "Bearer host-test-token" : null,
    );
  }
  await fireEvent.click(
    screen.getByRole("button", { name: "Sign in with new passkey" }),
  );
  const settings = await screen.findByRole("link", {
    name: "Open hosting settings",
  });
  expect(settings.getAttribute("href")).toBe("/hosting/settings");
  expect(get).toHaveBeenCalledTimes(1);
  expect(fetcher.mock.calls.filter(([path]) => path === "/api/auth/whoami"))
    .toHaveLength(2);
  expect(fetcher.mock.calls.filter(([path]) => path === "/api/hosting/access"))
    .toHaveLength(1);
  expect(
    fetcher.mock.calls.some(([path]) => String(path).includes("registration")),
  ).toBe(false);
});

for (const status of [401, 403, 503]) {
  test(`token rejection ${status} clears input and never falls back to registration`, async () => {
    const { fetcher, create } = boundary({ status });
    const log = vi.spyOn(console, "error");
    render(Setup);
    await enroll();
    await screen.findByRole("alert");
    expect(
      (screen.getByLabelText("Host setup token") as HTMLInputElement).value,
    ).toBe("");
    expect(screen.getByRole("alert").textContent).not.toContain(
      "host-test-token",
    );
    expect(log).not.toHaveBeenCalled();
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(create).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Sign in with new passkey" }))
      .toBeNull();
  });
}

for (
  const context of [{ admin: false }, { actor: false }, { failLogin: true }]
) {
  test(`unverified login or hosting access never exposes settings: ${JSON.stringify(context)}`, async () => {
    boundary(context);
    render(Setup);
    await enroll();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Sign in with new passkey" }),
    );
    await screen.findByRole("alert");
    expect(screen.queryByRole("link", { name: "Open hosting settings" }))
      .toBeNull();
  });
}

test("a different cookie actor cannot be accepted as the enrolled administrator", async () => {
  const { fetcher } = boundary();
  const original = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) =>
    String(input) === "/api/auth/whoami"
      ? Response.json({
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
        actor: {
          ...authUser,
          user_id: "other-user",
          auth_method: "browser_session",
        },
      })
      : original(input, init)
  );
  render(Setup);
  await enroll();
  await screen.findByRole("button", { name: "Sign in with new passkey" });
  await fireEvent.click(
    screen.getByRole("button", { name: "Sign in with new passkey" }),
  );
  await screen.findByRole("alert");
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
  expect(fetcher.mock.calls.some(([path]) => path === "/api/hosting/access"))
    .toBe(false);
});

test("leaving during the passkey prompt aborts enrollment and prevents later completion", async () => {
  const { create, fetcher } = boundary();
  let resolve!: (credential: Credential) => void;
  create.mockImplementation(() =>
    new Promise<Credential>((done) => {
      resolve = done;
    })
  );
  const { unmount } = render(Setup);
  await enroll();
  await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
  expect((screen.getByLabelText("Host setup token") as HTMLInputElement).value)
    .toBe("");
  const signal =
    (create.mock.calls[0] as unknown as [CredentialCreationOptions])[0].signal!;
  unmount();
  expect(signal.aborted).toBe(true);
  resolve(credentialFixture());
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(1));
  const { unmount: unmountAgain } = render(Setup);
  expect((screen.getByLabelText("Host setup token") as HTMLInputElement).value)
    .toBe("");
  unmountAgain();
});

test("completion response loss does not claim success and points to login before another enrollment", async () => {
  const { fetcher } = boundary();
  const original = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) => {
    if (String(input) === "/api/auth/passkeys/addition/complete") {
      throw new Error("host-test-token");
    }
    return original(input, init);
  });
  render(Setup);
  await enroll();
  await screen.findByRole("alert");
  expect(screen.getByRole("alert").textContent).toContain(
    "try signing in from Account",
  );
  expect(screen.getByRole("alert").textContent).not.toContain(
    "host-test-token",
  );
  expect((screen.getByLabelText("Host setup token") as HTMLInputElement).value)
    .toBe("");
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button", { name: "Sign in with new passkey" }))
    .toBeNull();
});

test("API token identity is not mistaken for a verified browser session", async () => {
  const { fetcher } = boundary();
  const original = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) =>
    String(input) === "/api/auth/whoami"
      ? Response.json({
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
        actor: { ...authUser, auth_method: "api_token" },
      })
      : original(input, init)
  );
  render(Setup);
  await enroll();
  await screen.findByRole("button", { name: "Sign in with new passkey" });
  await fireEvent.click(
    screen.getByRole("button", { name: "Sign in with new passkey" }),
  );
  await screen.findByRole("alert");
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
});
