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

function pageData(workspaceId = "workspace-1") {
  return {
    workspaceId,
    credentials: [
      { ...defaultCredential, workspace_id: workspaceId },
      { ...deployCredential, workspace_id: workspaceId },
    ],
    credentialsError: null,
    publicKeys: [{
      credential_id: "workspace-default",
      current_revision: 3,
      public_key_algorithm: "ssh-ed25519",
      public_key_fingerprint: "SHA256:default",
      public_key: "ssh-ed25519 AAAA-default",
    }],
    defaultPublicKeyError: null,
    hostTrusts: [{ ...hostTrust, workspace_id: workspaceId }],
    hostTrustsError: null,
    accessProjection: {
      workspace_id: workspaceId,
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

function pageProps(data = pageData()): PageProps {
  return {
    data,
    params: { workspaceId: data.workspaceId },
  } as unknown as PageProps;
}

function renderPage(data = pageData()) {
  return render(RepositoryAccessPage, pageProps(data));
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

test("fences a deferred editor while an unrelated host draft remains active after success", async () => {
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
  ).toBe(true);
  const deployRow = screen.getByText("Deploy key").closest("tr");
  if (!deployRow) throw new Error("missing deploy credential row");
  expect(
    (within(deployRow).getByRole("button", {
      name: "Rotate",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  const addHost = screen.getByRole("button", { name: "Add pinned key" });
  expect((addHost as HTMLButtonElement).disabled).toBe(false);
  await fireEvent.click(screen.getByRole("button", { name: "Generating…" }));
  expect(fetchMock).toHaveBeenCalledTimes(1);

  await fireEvent.click(addHost);
  const hostDraft = screen.getByRole("textbox", { name: "Host trust id" });
  await fireEvent.input(hostDraft, { target: { value: "draft-host" } });
  hostDraft.focus();

  createResponse.resolve(Response.json({
    ...deployCredential,
    credential_id: "generated",
    name: "Generated key",
    public_key_fingerprint: "SHA256:generated",
    current_revision: 1,
  }));
  expect(await screen.findByText("Generated key")).not.toBeNull();
  expect(screen.getByRole("heading", { name: "Add pinned host key" })).not
    .toBeNull();
  expect((hostDraft as HTMLInputElement).value).toBe("draft-host");
  expect(document.activeElement).toBe(hostDraft);
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

test("fences same-credential actions during a deferred row failure while unrelated controls remain enabled", async () => {
  const publicKeyResponse = deferred<Response>();
  const fetchMock = vi.fn(() => publicKeyResponse.promise);
  vi.stubGlobal("fetch", fetchMock);
  renderPage();

  const row = screen.getByText("Deploy key").closest("tr");
  if (!row) throw new Error("missing credential row");
  const rotate = within(row).getByRole("button", { name: "Rotate" });
  await fireEvent.click(rotate);
  const rotateSubmit = screen.getByRole("button", {
    name: "Rotate credential",
  });
  expect((rotateSubmit as HTMLButtonElement).disabled).toBe(false);

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
  const remove = within(row).getByRole("button", { name: "Delete" });
  expect((rotate as HTMLButtonElement).disabled).toBe(true);
  expect((remove as HTMLButtonElement).disabled).toBe(true);
  expect((rotateSubmit as HTMLButtonElement).disabled).toBe(true);
  await fireEvent.click(rotateSubmit);
  await fireEvent.click(rotate);
  await fireEvent.click(remove);
  expect(screen.getByRole("heading", { name: "Rotate Deploy key" })).not
    .toBeNull();
  await fireEvent.click(within(row).getByRole("button", { name: "Loading…" }));
  expect(fetchMock).toHaveBeenCalledTimes(1);

  publicKeyResponse.resolve(new Response(null, { status: 500 }));
  const alert = await within(row).findByRole("alert");
  expect(alert.textContent).toContain("Unable to load this public key");
  expect(screen.getAllByRole("alert")).toHaveLength(1);
  expect((rotateSubmit as HTMLButtonElement).disabled).toBe(false);
  expect(screen.getByRole("heading", { name: "Rotate Deploy key" })).not
    .toBeNull();
});

test("rerendering for another Workspace resets local state and fences stale responses", async () => {
  const stalePublicKey = deferred<Response>();
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("/api/w/workspace-1/")) return stalePublicKey.promise;
    if (url.includes("/api/w/workspace-2/")) {
      return Promise.resolve(new Response(null, { status: 500 }));
    }
    throw new Error(`unexpected request: ${url}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  const { rerender } = renderPage();

  await fireEvent.click(
    screen.getByRole("button", { name: "Generate credential" }),
  );
  await fireEvent.input(
    screen.getByRole("textbox", { name: "Credential id" }),
    {
      target: { value: "workspace-a-draft" },
    },
  );
  const workspaceARow = screen.getByText("Deploy key").closest("tr");
  if (!workspaceARow) throw new Error("missing Workspace A credential row");
  await fireEvent.click(
    within(workspaceARow).getByRole("button", { name: "Copy public key" }),
  );
  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

  const workspaceBData = {
    ...pageData("workspace-2"),
    credentials: [
      {
        ...defaultCredential,
        workspace_id: "workspace-2",
        name: "Workspace B default",
      },
      {
        ...deployCredential,
        workspace_id: "workspace-2",
        credential_id: "workspace-b-deploy",
        name: "Workspace B deploy",
      },
    ],
    publicKeys: [],
    hostTrusts: [{
      ...hostTrust,
      workspace_id: "workspace-2",
      host_trust_id: "workspace-b-host",
      hostname: "git.example.test",
    }],
    accessProjection: {
      workspace_id: "workspace-2",
      config_revision: 7,
      projection_digest: "sha256:workspace-b",
      bindings: [],
    },
  };
  await rerender(pageProps(workspaceBData));

  expect(await screen.findByText("Workspace B deploy")).not.toBeNull();
  expect(screen.queryByText("Deploy key")).toBeNull();
  expect(screen.queryByDisplayValue("workspace-a-draft")).toBeNull();
  expect(screen.getByText("git.example.test:22")).not.toBeNull();

  stalePublicKey.resolve(Response.json({
    credential_id: "deploy",
    current_revision: 2,
    public_key_algorithm: "ssh-ed25519",
    public_key_fingerprint: "SHA256:deploy",
    public_key: "ssh-ed25519 AAAA-stale",
  }));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(screen.queryByText("Public key copied.")).toBeNull();

  const workspaceBRow = screen.getByText("Workspace B deploy").closest("tr");
  if (!workspaceBRow) throw new Error("missing Workspace B credential row");
  await fireEvent.click(
    within(workspaceBRow).getByRole("button", { name: "Copy public key" }),
  );
  expect(await within(workspaceBRow).findByRole("alert")).not.toBeNull();
  expect(String(fetchMock.mock.calls[1]?.[0])).toContain(
    "/api/w/workspace-2/settings/repository-access/credentials/workspace-b-deploy/public-key",
  );
});

test("locks a rotation to its host and port and discloses explicit and automatic impact", async () => {
  const rotateResponse = deferred<Response>();
  const fetchMock = vi.fn((_input: RequestInfo | URL, _init?: RequestInit) =>
    rotateResponse.promise
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
  expect(
    (screen.getByRole("button", { name: "Saving…" }) as HTMLButtonElement)
      .disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", {
      name: "Add pinned key",
    }) as HTMLButtonElement).disabled,
  ).toBe(true);
  expect(
    (screen.getByRole("button", {
      name: "Generate credential",
    }) as HTMLButtonElement).disabled,
  ).toBe(false);

  rotateResponse.resolve(Response.json({
    ...hostTrust,
    host_key: "ssh-ed25519 AAAA-new",
    fingerprint: "SHA256:host-new",
    current_revision: 5,
  }));
  expect(await within(row).findByText(/Rotated the key for github.com:22/)).not
    .toBeNull();
  await waitFor(() =>
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Rotate key" }),
    )
  );
});
