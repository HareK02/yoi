// @vitest-environment happy-dom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { goto } from "$app/navigation";
import Catalog from "./+page.svelte";
import {
  createdWorkspace,
  creationResponse,
} from "#lib/workspace/api/workspace-catalog.test-fixtures.ts";

vi.mock("$app/navigation", () => ({ goto: vi.fn(async () => {}) }));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});
function mount() {
  return render(Catalog, {
    props: {
      data: {
        accessibleWorkspaces: [createdWorkspace],
        workspaceCatalogError: null,
        workspaces: [{ ...createdWorkspace, repositories: [] }],
        catalogError: null,
      },
    },
  });
}
function submit() {
  return fireEvent.submit(
    screen.getByRole("button", { name: /Create Workspace|Retry creation/ })
      .closest("form")!,
  );
}

test("blank repository form creates and opens a Workspace using a null repository response", async () => {
  const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
    Response.json(creationResponse()),
  );
  vi.stubGlobal("fetch", fetcher);
  mount();
  expect(
    screen.getByRole("link", { name: /New Workspace.*No repositories/ })
      .getAttribute("href"),
  ).toBe("/w/home-empty");
  expect(screen.queryByLabelText("Repository key")).toBeNull();
  await fireEvent.input(screen.getByLabelText("Workspace display name"), {
    target: { value: "  New Workspace  " },
  });
  await submit();
  await waitFor(() => expect(goto).toHaveBeenCalledWith("/w/home-empty"));
  expect(JSON.parse(String(fetcher.mock.calls[0][1]?.body))).toMatchObject({
    display_name: "New Workspace",
    repository: null,
  });
});

test("opting into a repository requires an explicit key and source and submits normalized values", async () => {
  const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
    Response.json(creationResponse()),
  );
  vi.stubGlobal("fetch", fetcher);
  mount();
  await fireEvent.click(
    screen.getByRole("checkbox", {
      name: "Add an initial repository (optional)",
    }),
  );
  const key = screen.getByLabelText("Repository key") as HTMLInputElement;
  const uri = screen.getByLabelText(
    "Initial repository absolute path or URI",
  ) as HTMLInputElement;
  expect(key.required && uri.required).toBe(true);
  expect(key.value).toBe("");
  await fireEvent.input(screen.getByLabelText("Workspace display name"), {
    target: { value: "New Workspace" },
  });
  await fireEvent.input(key, { target: { value: "platform" } });
  await fireEvent.input(uri, { target: { value: "  /srv/platform  " } });
  await fireEvent.input(screen.getByLabelText("Default ref"), {
    target: { value: "  main  " },
  });
  await submit();
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(String(fetcher.mock.calls[0][1]?.body)).repository).toEqual(
    { repository_key: "platform", uri: "/srv/platform", default_ref: "main" },
  );
});

test("unchanged empty creation retries reuse the operation key and accept deleted-repository replay", async () => {
  const fetcher = vi.fn<typeof fetch>()
    .mockResolvedValueOnce(
      Response.json({ message: "Temporary failure" }, { status: 503 }),
    )
    .mockResolvedValueOnce(Response.json(creationResponse(null, true)));
  vi.stubGlobal("fetch", fetcher);
  mount();
  await fireEvent.input(screen.getByLabelText("Workspace display name"), {
    target: { value: "New Workspace" },
  });
  await submit();
  expect(await screen.findByRole("alert")).toBeTruthy();
  await submit();
  await waitFor(() => expect(goto).toHaveBeenCalledWith("/w/home-empty"));
  expect(
    fetcher.mock.calls.map(([, init]) => JSON.parse(String(init?.body)))[0],
  ).toEqual(JSON.parse(String(fetcher.mock.calls[1][1]?.body)));
});

test("opting out ignores retained repository inputs and changes the creation operation", async () => {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async () =>
    Response.json({ message: "Temporary failure" }, { status: 503 })
  );
  vi.stubGlobal("fetch", fetcher);
  mount();
  const toggle = screen.getByRole("checkbox", {
    name: "Add an initial repository (optional)",
  });
  await fireEvent.input(screen.getByLabelText("Workspace display name"), {
    target: { value: "New Workspace" },
  });
  await fireEvent.click(toggle);
  await fireEvent.input(screen.getByLabelText("Repository key"), {
    target: { value: "platform" },
  });
  await fireEvent.input(
    screen.getByLabelText("Initial repository absolute path or URI"),
    { target: { value: "/srv/platform" } },
  );
  await submit();
  await screen.findByRole("alert");
  await fireEvent.click(toggle);
  await submit();
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  const bodies = fetcher.mock.calls.map(([, init]) =>
    JSON.parse(String(init?.body))
  );
  expect(bodies[1].repository).toBeNull();
  expect(bodies[1].operation_key).not.toBe(bodies[0].operation_key);
});
