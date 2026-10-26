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
  browserSession,
  observeBrowserSession,
} from "#lib/workspace/auth/session-state.ts";
import { SESSION_CHANGED_EVENT } from "#lib/workspace/auth/hosting.ts";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "#lib/workspace/auth/api.test-fixtures.ts";

afterEach(() => {
  cleanup();
  browserSession.invalidate();
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
  const fetcher = vi.fn<typeof fetch>().mockImplementation(
    async (input, init) => {
      const path = String(input);
      if (
        path === "/api/auth/whoami" &&
        new Headers(init?.headers).has("authorization")
      ) {
        return status === 200
          ? Response.json({
            actor: { ...authUser, auth_method: "api_token" },
            authentication_reference: "fixture-bearer-authentication",
            session: { kind: "authenticated", expires_at: null },
          })
          : new Response("host-test-token", { status });
      }
      if (path === "/api/auth/whoami") {
        return Response.json({
          authentication_reference: actor ? "fixture-authentication" : null,
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
        return Response.json({
          user: authUser,
          authentication_reference: "fixture-authentication",
        });
      }
      if (path === "/api/auth/passkeys/login/options") {
        return Response.json(loginOptions);
      }
      if (path === "/api/auth/passkeys/login/complete") {
        return failLogin ? new Response(null, { status: 401 }) : Response.json({
          user: authUser,
          authentication_reference: "fixture-authentication",
        });
      }
      throw new Error(`Unexpected API: ${path}`);
    },
  );
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
    expect(init?.credentials).toBe("omit");
    expect(new Headers(init?.headers).has("cookie")).toBe(false);
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer host-test-token",
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
    .toHaveLength(6);
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
    String(input) === "/api/auth/whoami" &&
      !new Headers(init?.headers).has("authorization")
      ? Response.json({
        authentication_reference: "fixture-authentication",
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
  await waitFor(() =>
    expect(fetcher.mock.calls.map(([path]) => path)).toEqual([
      "/api/auth/whoami",
      "/api/auth/passkeys/addition/options",
    ])
  );
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
  expect(fetcher).toHaveBeenCalledTimes(3);
  expect(screen.queryByRole("button", { name: "Sign in with new passkey" }))
    .toBeNull();
});

test("API token identity is not mistaken for a verified browser session", async () => {
  const { fetcher } = boundary();
  const original = fetcher.getMockImplementation()!;
  fetcher.mockImplementation(async (input, init) =>
    String(input) === "/api/auth/whoami"
      ? Response.json({
        authentication_reference: "fixture-authentication",
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

function gate<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
const setupActor = { ...authUser, auth_method: "browser_session" };
const replacedActor = {
  ...setupActor,
  user_id: "other-user",
  account_id: "other-account",
  handle: "bob",
};
function identityResponse(actor: typeof setupActor | null) {
  return Response.json({
    actor,
    authentication_reference: actor ? "fixture-authentication" : null,
    session: actor
      ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
      : { kind: "anonymous" },
  });
}
async function changedInAnotherTab(actor: typeof setupActor | null) {
  window.dispatchEvent(
    new StorageEvent("storage", {
      key: "yoi.browser-auth-generation",
      newValue: JSON.stringify({
        id: crypto.randomUUID(),
        reason: actor ? "authentication" : "logout",
      }),
    }),
  );
  await browserSession.refresh();
}
for (const phase of ["login", "whoami", "access", "confirmation"] as const) {
  for (const replacement of [null, replacedActor]) {
    test(`newer ${replacement ? "actor" : "logout"} while setup ${phase} is pending cannot publish old verified success`, async () => {
      const { fetcher } = boundary();
      const original = fetcher.getMockImplementation()!;
      const started = gate<void>();
      const pending = gate<Response>();
      let current: typeof setupActor | null = setupActor;
      let loginCompleted = false;
      let accessRead = false;
      let held = false;
      fetcher.mockImplementation((input, init) => {
        const path = String(input);
        // Bearer identity resolution belongs to enrollment, never the cookie-login gates.
        if (new Headers(init?.headers).has("authorization")) {
          return original(input, init);
        }
        if (path === "/api/auth/passkeys/login/complete") loginCompleted = true;
        if (path === "/api/hosting/access") accessRead = true;
        if (path === "/api/auth/whoami") {
          if (
            !held && loginCompleted && (
              (phase === "whoami" && !accessRead) ||
              (phase === "confirmation" && accessRead)
            )
          ) {
            held = true;
            started.resolve();
            return pending.promise;
          }
          return Promise.resolve(identityResponse(current));
        }
        if (
          (phase === "access" && path === "/api/hosting/access") ||
          (phase === "login" && path === "/api/auth/passkeys/login/complete")
        ) {
          started.resolve();
          return pending.promise;
        }
        return original(input, init);
      });
      observeBrowserSession();
      render(Setup);
      await enroll();
      await fireEvent.click(
        await screen.findByRole("button", { name: "Sign in with new passkey" }),
      );
      await started.promise;
      const notify = vi.fn();
      window.addEventListener(SESSION_CHANGED_EVENT, notify);
      try {
        current = replacement;
        await changedInAnotherTab(current);
        const acceptedGeneration = browserSession.authenticationGeneration;
        pending.resolve(
          phase === "access"
            ? Response.json({ is_admin: true })
            : phase === "login"
            ? Response.json({
              user: authUser,
              authentication_reference: "fixture-authentication",
            })
            : identityResponse(setupActor),
        );
        await waitFor(() =>
          expect(
            (screen.getByRole("button", {
              name: "Sign in with new passkey",
            }) as HTMLButtonElement).disabled,
          ).toBe(false)
        );
        expect(screen.queryByRole("link", { name: "Open hosting settings" }))
          .toBeNull();
        expect(
          screen.queryByText(
            "Passkey login and hosting administrator access verified.",
          ),
        ).toBeNull();
        expect(notify).not.toHaveBeenCalled();
        expect(browserSession.authenticationGeneration).toBe(
          acceptedGeneration,
        );
        expect(browserSession.snapshot).toMatchObject({
          kind: "ready",
          value: { actor: current },
        });
      } finally {
        window.removeEventListener(SESSION_CHANGED_EVENT, notify);
      }
    });
  }
}
for (const replacement of [null, replacedActor]) {
  test(`setup verified success is removed on subsequent ${replacement ? "actor replacement" : "logout"}`, async () => {
    const { fetcher } = boundary();
    observeBrowserSession();
    render(Setup);
    await enroll();
    await fireEvent.click(
      await screen.findByRole("button", { name: "Sign in with new passkey" }),
    );
    await screen.findByRole("link", { name: "Open hosting settings" });
    const original = fetcher.getMockImplementation()!;
    fetcher.mockImplementation((input, init) =>
      String(input) === "/api/auth/whoami"
        ? Promise.resolve(identityResponse(replacement))
        : original(input, init)
    );
    await changedInAnotherTab(replacement);
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Open hosting settings" }))
        .toBeNull()
    );
    expect(
      screen.queryByText(
        "Passkey login and hosting administrator access verified.",
      ),
    ).toBeNull();
    expect(
      (screen.getByRole("button", {
        name: "Sign in with new passkey",
      }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });
}

test("setup logout during native login prompt aborts and never submits the late credential", async () => {
  const { get, fetcher } = boundary();
  const prompted = gate<void>();
  const credential = gate<Credential>();
  get.mockImplementation(() => {
    prompted.resolve();
    return credential.promise;
  });
  render(Setup);
  await enroll();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Sign in with new passkey" }),
  );
  await prompted.promise;
  await changedInAnotherTab(null);
  const signal = (get.mock.calls[0] as unknown as [CredentialRequestOptions])[0]
    .signal!;
  expect(signal.aborted).toBe(true);
  credential.resolve(credentialFixture(true));
  await screen.findByRole("alert");
  expect(
    fetcher.mock.calls.some(([path]) =>
      path === "/api/auth/passkeys/login/complete"
    ),
  ).toBe(false);
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
});

test("setup final confirmation rejects replacement session for the same owner", async () => {
  const { fetcher } = boundary();
  const request = fetcher.getMockImplementation()!;
  let accessRead = false;
  fetcher.mockImplementation(async (input, init) => {
    if (String(input) === "/api/hosting/access") accessRead = true;
    if (String(input) === "/api/auth/whoami" && accessRead) {
      return Response.json({
        actor: setupActor,
        authentication_reference: "same-owner-different-session",
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      });
    }
    return await request(input, init);
  });
  render(Setup);
  await enroll();
  await fireEvent.click(
    await screen.findByRole("button", { name: "Sign in with new passkey" }),
  );
  await screen.findByRole("alert");
  expect(screen.queryByRole("link", { name: "Open hosting settings" }))
    .toBeNull();
});

for (
  const phase of [
    "initial",
    "options",
    "prompt",
    "pre-complete",
    "complete",
    "whoami",
    "access",
    "confirmation",
  ] as const
) {
  for (const reason of ["logout", "authentication"] as const) {
    test(`setup fences shared ${reason} stamp during ${phase} before event delivery`, async () => {
      const { fetcher, get } = boundary();
      const request = fetcher.getMockImplementation()!;
      const started = gate<void>();
      const pending = gate<void>();
      let optionsRead = false,
        promptReturned = false,
        completionSent = false,
        accessRead = false,
        held = false;
      const hold = async () => {
        held = true;
        started.resolve();
        await pending.promise;
      };
      get.mockImplementation(async () => {
        if (phase === "prompt") await hold();
        promptReturned = true;
        return credentialFixture(true);
      });
      fetcher.mockImplementation(async (input, init) => {
        if (new Headers(init?.headers).has("authorization")) {
          return await request(input, init);
        }
        const path = String(input);
        const point = path === "/api/auth/whoami"
          ? !optionsRead
            ? "initial"
            : !completionSent && promptReturned
            ? "pre-complete"
            : accessRead
            ? "confirmation"
            : "whoami"
          : path.endsWith("/login/options")
          ? "options"
          : path.endsWith("/login/complete")
          ? "complete"
          : path === "/api/hosting/access"
          ? "access"
          : null;
        if (point === "options") optionsRead = true;
        if (point === "complete") completionSent = true;
        if (point === "access") accessRead = true;
        const response = await request(input, init);
        if (!held && point === phase) await hold();
        return response;
      });
      render(Setup);
      await enroll();
      await fireEvent.click(
        await screen.findByRole("button", { name: "Sign in with new passkey" }),
      );
      await started.promise;
      const generation = browserSession.authenticationGeneration;
      const stamp = JSON.stringify({ id: crypto.randomUUID(), reason });
      // 同一documentのsetItemはstorage eventを配送しない。generationを直接変えず順序を固定する。
      localStorage.setItem("yoi.browser-auth-generation", stamp);
      expect(browserSession.authenticationGeneration).toBe(generation);
      pending.resolve();
      await screen.findByRole("alert");
      await waitFor(() =>
        expect(
          (screen.getByRole("button", {
            name: "Sign in with new passkey",
          }) as HTMLButtonElement).disabled,
        ).toBe(false)
      );
      expect(
        fetcher.mock.calls.filter(([path]) =>
          path === "/api/auth/passkeys/login/complete"
        ),
      ).toHaveLength(
        ["complete", "whoami", "access", "confirmation"].includes(phase)
          ? 1
          : 0,
      );
      expect(screen.queryByRole("link", { name: "Open hosting settings" }))
        .toBeNull();
      // 古いcatchが新tabのstampを自身のsessionMutationで上書きしない。
      expect(browserSession.authenticationStamp).toBe(stamp);
    });
  }
}
