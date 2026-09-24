// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import RepositoryAccessPage from "./+page.svelte";
import type { PageProps } from "./$types";

const defaultCredential = {
  credential_id: "workspace-default",
  workspace_id: "workspace-1",
  name: "Workspace default",
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:default",
  current_revision: 3,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  rotated_at: null,
  referenced_repositories: [],
};

const deployCredential = {
  credential_id: "deploy",
  workspace_id: "workspace-1",
  name: "Deploy key",
  public_key_algorithm: "ssh-ed25519",
  public_key_fingerprint: "SHA256:deploy",
  current_revision: 2,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  rotated_at: null,
  referenced_repositories: [],
};

const hostTrust = {
  host_trust_id: "github-com",
  workspace_id: "workspace-1",
  hostname: "github.com",
  port: 22,
  key_algorithm: "ssh-ed25519",
  host_key: "ssh-ed25519 AAAA-current",
  fingerprint: "SHA256:host-current",
  current_revision: 4,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  referenced_repositories: [],
};

function pageData() {
  return {
    workspaceId: "workspace-1",
    credentials: [defaultCredential, deployCredential],
    credentialsError: null,
    publicKeys: [{
      credential_id: "workspace-default",
      current_revision: 3,
      public_key_algorithm: "ssh-ed25519",
      public_key_fingerprint: "SHA256:default",
      public_key: "ssh-ed25519 AAAA-default",
    }],
    defaultPublicKeyError: null,
    hostTrusts: [hostTrust],
    hostTrustsError: null,
    accessProjection: {
      workspace_id: "workspace-1",
      config_revision: 1,
      projection_digest: "sha256:projection",
      bindings: [],
    },
    accessProjectionError: null,
    repositories: {
      items: [{
        repository_key: "platform",
        source: { kind: "ssh", uri: "git@github.com:team/platform.git" },
      }],
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function renderPage() {
  const props = {
    data: pageData(),
    params: { workspaceId: "workspace-1" },
  } as unknown as PageProps;
  return render(RepositoryAccessPage, props);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

test("opens disclosures with focus, focuses invalid fields, and restores trigger focus on cancel", async () => {
  renderPage();
  const trigger = screen.getByRole("button", { name: "Generate credential" });

  expect(screen.queryByRole("heading", { name: "Generate SSH credential" }))
    .toBeNull();
  trigger.focus();
  await fireEvent.click(trigger);

  const idInput = screen.getByRole("textbox", { name: "Credential id" });
  await waitFor(() => expect(document.activeElement).toBe(idInput));
  await fireEvent.click(
    screen.getAllByRole("button", { name: "Generate credential" })[1],
  );
  expect(await screen.findByText("Credential id is required.")).not.toBeNull();
  expect(document.activeElement).toBe(idInput);

  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() =>
    expect(screen.queryByRole("heading", { name: "Generate SSH credential" }))
      .toBeNull()
  );
  expect(document.activeElement).toBe(trigger);
});

test("fences a deferred submission without disabling unrelated controls and restores focus after success", async () => {
  const createResponse = deferred<Response>();
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/credentials/generate")) return createResponse.promise;
    if (url.endsWith("/credentials/generated/public-key")) {
      return Promise.resolve(Response.json({
        credential_id: "generated",
        current_revision: 1,
        public_key_algorithm: "ssh-ed25519",
        public_key_fingerprint: "SHA256:generated",
        public_key: "ssh-ed25519 AAAA-generated",
      }));
    }
    throw new Error(`unexpected request: ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  renderPage();

  const trigger = screen.getByRole("button", { name: "Generate credential" });
  trigger.focus();
  await fireEvent.click(trigger);
  await fireEvent.input(
    screen.getByRole("textbox", { name: "Credential id" }),
    { target: { value: "generated" } },
  );
  await fireEvent.input(screen.getByRole("textbox", { name: "Name" }), {
    target: { value: "Generated key" },
  });
  const submit =
    screen.getAllByRole("button", { name: "Generate credential" })[1];
  await fireEvent.click(submit);

  await waitFor(() =>
    expect(
      (screen.getByRole("button", { name: "Generating…" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true)
  );
  expect(
    (screen.getByRole("button", {
      name: "Import credential",
    }) as HTMLButtonElement).disabled,
  ).toBe(false);
  expect(
    (screen.getByRole("button", {
      name: "Add pinned key",
    }) as HTMLButtonElement).disabled,
  ).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Generating…" }));
  expect(fetchMock).toHaveBeenCalledTimes(1);

  createResponse.resolve(Response.json({
    ...deployCredential,
    credential_id: "generated",
    name: "Generated key",
    public_key_fingerprint: "SHA256:generated",
    current_revision: 1,
  }));
  expect(await screen.findByText("Generated key")).not.toBeNull();
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

test("keeps a deferred row failure on that row while unrelated actions remain enabled", async () => {
  const publicKeyResponse = deferred<Response>();
  const fetchMock = vi.fn(() => publicKeyResponse.promise);
  vi.stubGlobal("fetch", fetchMock);
  renderPage();

  const row = screen.getByText("Deploy key").closest("tr");
  if (!row) throw new Error("missing credential row");
  const copy = within(row).getByRole("button", { name: "Copy public key" });
  await fireEvent.click(copy);

  await waitFor(() =>
    expect(
      (within(row).getByRole("button", {
        name: "Loading…",
      }) as HTMLButtonElement).disabled,
    ).toBe(true)
  );
  expect(
    (screen.getByRole("button", {
      name: "Generate credential",
    }) as HTMLButtonElement).disabled,
  ).toBe(false);
  expect(
    (screen.getByRole("button", {
      name: "Add pinned key",
    }) as HTMLButtonElement).disabled,
  ).toBe(false);
  await fireEvent.click(within(row).getByRole("button", { name: "Loading…" }));
  expect(fetchMock).toHaveBeenCalledTimes(1);

  publicKeyResponse.resolve(new Response(null, { status: 500 }));
  const alert = await within(row).findByRole("alert");
  expect(alert.textContent).toContain("Unable to load this public key");
  expect(screen.getAllByRole("alert")).toHaveLength(1);
});

test("locks a rotation to its host and port and discloses explicit and automatic impact", async () => {
  const fetchMock = vi.fn((_input: RequestInfo | URL, init?: RequestInit) =>
    Promise.resolve(Response.json({
      ...hostTrust,
      host_key: "ssh-ed25519 AAAA-new",
      fingerprint: "SHA256:host-new",
      current_revision: 5,
    }))
  );
  vi.stubGlobal("fetch", fetchMock);
  renderPage();

  const row = screen.getByText("github.com:22").closest("tr");
  if (!row) throw new Error("missing host trust row");
  const trigger = within(row).getByRole("button", { name: "Rotate key" });
  trigger.focus();
  await fireEvent.click(trigger);

  const hostname = screen.getByRole("textbox", { name: "Hostname" });
  const port = screen.getByRole("spinbutton", { name: "Port" });
  const hostKeyInput = screen.getByRole("textbox", {
    name: /^OpenSSH public host key/,
  });
  expect((hostname as HTMLInputElement).disabled).toBe(true);
  expect((port as HTMLInputElement).disabled).toBe(true);
  expect(screen.getByText(/keeps the endpoint fixed/).textContent).toContain(
    "may also select it automatically",
  );
  expect(screen.getByText("Current fingerprint").parentElement?.textContent)
    .toContain("SHA256:host-current");
  await waitFor(() => expect(document.activeElement).toBe(hostKeyInput));

  await fireEvent.input(hostKeyInput, {
    target: { value: "ssh-ed25519 AAAA-new" },
  });
  await fireEvent.click(screen.getByRole("button", { name: "Save new key" }));
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
  const body = JSON.parse(String(fetchMock.mock.calls[0]?.[1]?.body));
  expect(body.hostname).toBe("github.com");
  expect(body.port).toBe(22);
  expect(body.expected_revision).toBe(4);
  expect(await within(row).findByText(/Rotated the key for github.com:22/)).not
    .toBeNull();
  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Rotate key" }),
    )
  );
});
