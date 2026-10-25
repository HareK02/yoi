// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { browserSession } from "#lib/workspace/auth/session-state.ts";
import { afterEach, expect, test, vi } from "vitest";
import Settings from "./+page.svelte";
import { authUser } from "#lib/workspace/auth/api.test-fixtures.ts";

afterEach(() => {
  cleanup();
  browserSession.invalidate();
  vi.unstubAllGlobals();
});

function boundary(
  persona: "anonymous" | "ordinary" | "admin",
  enabled = true,
  saveStatus = 200,
) {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(
    async (input, init) => {
      const path = String(input);
      if (path === "/api/auth/whoami") {
        return Response.json({
          authentication_reference: persona !== "anonymous" ? "fixture-authentication" : null,
          session: persona !== "anonymous"
            ? { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" }
            : { kind: "anonymous" },
          actor: persona === "anonymous"
            ? null
            : { ...authUser, auth_method: "browser_session" },
        });
      }
      if (path === "/api/hosting/access") {
        return Response.json({ is_admin: persona === "admin" });
      }
      if (path === "/api/hosting/settings") {
        if (persona !== "admin") return new Response(null, { status: 403 });
        if (init?.method === "PUT") {
          if (saveStatus !== 200) {
            return new Response(null, { status: saveStatus });
          }
          enabled = JSON.parse(String(init.body)).self_registration_enabled;
        }
        return Response.json({ self_registration_enabled: enabled });
      }
      throw new Error(`Unexpected API ${path}`);
    },
  );
  vi.stubGlobal("fetch", fetcher);
  return fetcher;
}

for (const persona of ["anonymous", "ordinary"] as const) {
  test(`${persona} direct hosting screen contains no settings controls or privileged requests`, async () => {
    const fetcher = boundary(persona);
    render(Settings);
    await screen.findByText(
      persona === "anonymous"
        ? "Sign in with a hosting administrator account to view these settings."
        : "Hosting administrator access is required.",
    );
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(
      fetcher.mock.calls.some(([path]) => path === "/api/hosting/settings"),
    ).toBe(false);
  });
}

for (const enabled of [true, false]) {
  test(`admin reads policy ${enabled}, edits with keyboard focus, cancels and saves confirmed value`, async () => {
    const fetcher = boundary("admin", enabled);
    render(Settings);
    const edit = await screen.findByRole("button", {
      name: "Edit registration policy",
    });
    expect(screen.getByText(enabled ? "Enabled" : "Disabled")).toBeTruthy();
    expect(screen.queryByRole("checkbox")).toBeNull();
    await fireEvent.click(edit);
    let checkbox = screen.getByRole("checkbox") as HTMLInputElement;
    await waitFor(() => expect(document.activeElement).toBe(checkbox));
    expect(checkbox.checked).toBe(enabled);
    expect(
      (screen.getByRole("button", {
        name: "Save settings",
      }) as HTMLButtonElement).disabled,
    ).toBe(true);
    await fireEvent.click(checkbox);
    await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Edit registration policy" }),
      )
    );
    await fireEvent.click(
      screen.getByRole("button", { name: "Edit registration policy" }),
    );
    checkbox = screen.getByRole("checkbox") as HTMLInputElement;
    expect(checkbox.checked).toBe(enabled);
    await fireEvent.click(checkbox);
    await fireEvent.click(
      screen.getByRole("button", { name: "Save settings" }),
    );
    await screen.findByText("Hosting settings saved.");
    expect(screen.getByText(enabled ? "Disabled" : "Enabled")).toBeTruthy();
    const writes = fetcher.mock.calls.filter(([, init]) =>
      init?.method === "PUT"
    );
    expect(writes).toHaveLength(1);
    expect(JSON.parse(String(writes[0][1]?.body))).toEqual({
      self_registration_enabled: !enabled,
    });
  });
}

for (const status of [401, 403, 503]) {
  test(`save rejection ${status} never claims success or automatically retries`, async () => {
    const fetcher = boundary("admin", true, status);
    render(Settings);
    await fireEvent.click(
      await screen.findByRole("button", { name: "Edit registration policy" }),
    );
    await fireEvent.click(screen.getByRole("checkbox"));
    await fireEvent.click(
      screen.getByRole("button", { name: "Save settings" }),
    );
    if (status === 503) {
      await screen.findByText(
        "Save could not be confirmed. Reload settings before making another change.",
      );
      expect(screen.getByText("Enabled")).toBeTruthy();
      expect(
        (screen.getByRole("button", {
          name: "Save settings",
        }) as HTMLButtonElement).disabled,
      ).toBe(true);
      await fireEvent.click(
        screen.getByRole("button", { name: "Reload settings" }),
      );
      await screen.findByRole("button", { name: "Edit registration policy" });
    } else {
      await screen.findByText(
        status === 401
          ? "Sign in with a hosting administrator account to view these settings."
          : "Hosting administrator access is required.",
      );
      expect(screen.queryByRole("checkbox")).toBeNull();
      expect(screen.queryByRole("button")).toBeNull();
    }
    expect(screen.queryByText("Hosting settings saved.")).toBeNull();
    expect(fetcher.mock.calls.filter(([, init]) => init?.method === "PUT"))
      .toHaveLength(1);
  });
}

test("unconfirmed access and settings failures show no settings controls and offer retry", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(null, { status: 503 })),
  );
  render(Settings);
  await screen.findByText("Hosting settings could not be loaded.");
  expect(screen.queryByRole("checkbox")).toBeNull();
  expect(screen.queryByRole("button", { name: "Edit registration policy" }))
    .toBeNull();
  expect(screen.getByRole("button", { name: "Retry loading settings" }))
    .toBeTruthy();
});
