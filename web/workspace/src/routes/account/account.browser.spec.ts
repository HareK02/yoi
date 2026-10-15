// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import Account from "./+page.svelte";
import {
  authUser,
  credentialFixture,
  loginOptions,
  registrationOptions,
} from "#lib/workspace/auth/api.test-fixtures.ts";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

// UIから本番HTTP adapterまでを接続し、fetch/ブラウザーcredential境界だけを代替する。
function boundary(
  authenticated: boolean,
  completeStatus = 200,
  policy: boolean | "error" | "loading" = true,
) {
  const create = vi.fn(async () => credentialFixture());
  const get = vi.fn(async () => credentialFixture(true));
  vi.stubGlobal("navigator", { credentials: { create, get } });
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input) => {
    const path = String(input);
    if (path === "/api/auth/whoami") {
      return Response.json({
        actor: authenticated
          ? { ...authUser, auth_method: "browser_session" }
          : null,
      });
    }
    if (path === "/api/auth/registration-policy") {
      if (policy === "loading") return await new Promise<Response>(() => {});
      if (policy === "error") return new Response(null, { status: 503 });
      return Response.json({ self_registration_enabled: policy });
    }
    if (path === "/api/auth/passkeys/login/options") {
      return Response.json(loginOptions);
    }
    if (path.endsWith("/options")) return Response.json(registrationOptions);
    if (path.endsWith("/complete")) {
      if (completeStatus !== 200) {
        if (completeStatus === 403 && path.includes("/registration/")) {
          policy = false;
        }
        return new Response(null, { status: completeStatus });
      }
      authenticated = true;
      return Response.json({ user: authUser });
    }
    throw new Error(`Unexpected API: ${path}`);
  });
  vi.stubGlobal("fetch", fetcher);
  return { fetcher, create, get };
}

function posted(fetcher: ReturnType<typeof boundary>["fetcher"]) {
  return fetcher.mock.calls.filter(([, init]) => init?.method === "POST").map((
    [path, init],
  ) => ({
    path: String(path),
    body: JSON.parse(String(init!.body)),
    credentials: init!.credentials,
  }));
}

test("anonymous existing Account page registers a new account via registration endpoints", async () => {
  const { fetcher, create } = boundary(false);
  render(Account);
  const button = await screen.findByRole("button", {
    name: "Register new account",
  });
  await waitFor(() =>
    expect((button as HTMLButtonElement).disabled).toBe(false)
  );
  await fireEvent.input(
    screen.getByLabelText("User handle (new registration or login)"),
    { target: { value: " alice " } },
  );
  await fireEvent.input(
    screen.getByLabelText("Display name (new account only)"),
    { target: { value: "Alice" } },
  );
  await fireEvent.click(button);
  await screen.findByText("New account registered.");
  const requests = posted(fetcher);
  expect(requests.map((request) => request.path)).toEqual([
    "/api/auth/passkeys/registration/options",
    "/api/auth/passkeys/registration/complete",
  ]);
  expect(requests[0].body).toEqual({
    handle: "alice",
    display_name: "Alice",
    browser_origin: window.location.origin,
  });
  expect(requests[1].body.challenge_id).toBe(registrationOptions.challenge_id);
  expect(create).toHaveBeenCalledOnce();
});

test("authenticated Account locks identity fields and adds only through self-only addition endpoints", async () => {
  const { fetcher, create } = boundary(true, 200, false);
  render(Account);
  const button = await screen.findByRole("button", {
    name: "Add passkey to alice",
  });
  expect(
    (screen.getByLabelText("Current user handle") as HTMLInputElement).disabled,
  ).toBe(true);
  expect(
    (screen.getByLabelText(
      "Display name (new account only)",
    ) as HTMLInputElement).disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", {
      name: "Log in with passkey",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  await fireEvent.click(button);
  await screen.findByText("Passkey added to your account.");
  const requests = posted(fetcher);
  expect(requests.map((request) => request.path)).toEqual([
    "/api/auth/passkeys/addition/options",
    "/api/auth/passkeys/addition/complete",
  ]);
  expect(requests[0].body).toEqual({ browser_origin: window.location.origin });
  expect(Object.keys(requests[1].body).sort()).toEqual([
    "challenge_id",
    "credential",
  ]);
  expect(requests.map((request) => request.credentials)).toEqual([
    "same-origin",
    "same-origin",
  ]);
  expect(create).toHaveBeenCalledOnce();
});

test("addition completion rejection is visible and never retries as new registration", async () => {
  const { fetcher } = boundary(true, 401);
  render(Account);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Add passkey to alice" }),
  );
  await screen.findByText("Auth request failed (401).");
  expect(posted(fetcher).map((request) => request.path)).toEqual([
    "/api/auth/passkeys/addition/options",
    "/api/auth/passkeys/addition/complete",
  ]);
});

test("blank handle disables anonymous registration and login", async () => {
  boundary(false);
  render(Account);
  await screen.findByText("No browser session is active.");
  await fireEvent.input(
    screen.getByLabelText("User handle (new registration or login)"),
    { target: { value: "  " } },
  );
  expect(
    (screen.getByRole("button", {
      name: "Register new account",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", {
      name: "Log in with passkey",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
});

test("unknown session never enables registration or addition", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(null, { status: 503 })),
  );
  render(Account);
  await screen.findByText("Auth request failed (503).");
  expect(
    (screen.getByLabelText(
      "User handle (new registration or login)",
    ) as HTMLInputElement).disabled,
  ).toBe(true);
  expect(screen.queryByRole("button", { name: /Add passkey/ })).toBeNull();
  expect(screen.queryByRole("button", { name: "Register new account" }))
    .toBeNull();
});

for (const policy of [false, "error", "loading"] as const) {
  test(`policy ${policy} still permits the existing passkey login HTTP flow`, async () => {
    const { fetcher, get, create } = boundary(false, 200, policy);
    render(Account);
    await screen.findByText("No browser session is active.");
    await fireEvent.click(
      screen.getByRole("button", { name: "Log in with passkey" }),
    );
    await screen.findByText("Logged in.");
    expect(posted(fetcher).map((request) => request.path)).toEqual([
      "/api/auth/passkeys/login/options",
      "/api/auth/passkeys/login/complete",
    ]);
    expect(get).toHaveBeenCalledOnce();
    expect(create).not.toHaveBeenCalled();
  });
}

test("registration disabled during WebAuthn completion shows rejection and reloads policy without fallback", async () => {
  const { fetcher } = boundary(false, 403, true);
  render(Account);
  await fireEvent.click(
    await screen.findByRole("button", { name: "Register new account" }),
  );
  await screen.findByText("Auth request failed (403).");
  await screen.findByText(/New account registration is disabled/);
  expect(screen.queryByRole("button", { name: "Register new account" }))
    .toBeNull();
  expect(screen.queryByText("New account registered.")).toBeNull();
  expect(posted(fetcher).map((request) => request.path)).toEqual([
    "/api/auth/passkeys/registration/options",
    "/api/auth/passkeys/registration/complete",
  ]);
});

for (const policy of [false, "error", "loading"] as const) {
  test(`registration policy ${policy} hides registration without disabling login`, async () => {
    const { fetcher } = boundary(false, 200, policy);
    render(Account);
    await screen.findByText("No browser session is active.");
    if (policy === false) {
      await screen.findByText(/New account registration is disabled/);
    }
    if (policy === "error") {
      await screen.findByText("Registration availability could not be loaded.");
    }
    if (policy === "loading") {
      await screen.findByText("Checking registration availability…");
    }
    expect(screen.queryByRole("button", { name: "Register new account" }))
      .toBeNull();
    expect(
      (screen.getByRole("button", {
        name: "Log in with passkey",
      }) as HTMLButtonElement).disabled,
    ).toBe(false);
    expect(posted(fetcher)).toEqual([]);
  });
}

for (const policy of ["error", "loading"] as const) {
  test(`policy ${policy} does not block actor-self passkey addition`, async () => {
    const { fetcher } = boundary(true, 200, policy);
    render(Account);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Add passkey to alice" }),
    );
    await screen.findByText("Passkey added to your account.");
    expect(posted(fetcher).map((request) => request.path)).toEqual([
      "/api/auth/passkeys/addition/options",
      "/api/auth/passkeys/addition/complete",
    ]);
  });
}
