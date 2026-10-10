// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { DriveMutationRequest } from "#lib/generated/drive-api.ts";
import DrivePage from "./DrivePage.svelte";
import { entry } from "../drive/test-fixtures.ts";
vi.mock("$app/navigation", () => ({ beforeNavigate: vi.fn() }));
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

test("A late image decode error cannot replace the newly selected image preview", async () => {
  let sequence = 0;
  const create = vi.spyOn(URL, "createObjectURL").mockImplementation(() =>
    `blob:http://localhost/image-${++sequence}`
  );
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const id = url.searchParams.get("id") ?? "1";
      if (url.pathname.endsWith("/download")) {
        return new Response(new Uint8Array(8));
      }
      if (id === "1") return Response.json(entry("1", "alpha", "1", "folder"));
      return Response.json({
        ...entry(id),
        name: `image-${id}.png`,
        content_type: "image/png",
        size: 8,
      });
    }),
  );
  const view = render(DrivePage, { workspaceId: "alpha", nodeId: "2" });
  await waitFor(() =>
    expect(view.container.querySelector("img")?.getAttribute("src")).toBe(
      "blob:http://localhost/image-1",
    )
  );
  const oldImage = view.container.querySelector("img")!;
  await view.rerender({ workspaceId: "alpha", nodeId: "3" });
  await waitFor(() =>
    expect(view.container.querySelector("img")?.getAttribute("src")).toBe(
      "blob:http://localhost/image-2",
    )
  );
  expect(view.container.querySelector("img")).not.toBe(oldImage);
  await fireEvent.error(oldImage);
  expect(view.container.textContent).not.toContain(
    "Image could not be decoded",
  );
  await fireEvent.error(view.container.querySelector("img")!);
  expect(view.container.textContent).toContain("Image could not be decoded");
  expect(create).toHaveBeenCalledTimes(2);
  expect(revoke).toHaveBeenCalledWith("blob:http://localhost/image-1");
});

test("Refreshing metadata never silently rebases an open rename form onto another writer’s saved content", async () => {
  let last_mutation_id = "1";
  const mutations: DriveMutationRequest[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const id = url.searchParams.get("id") ?? "1";
      if (url.pathname.endsWith("/mutate")) {
        mutations.push(JSON.parse(String(init?.body)));
        return Response.json({
          code: "conflict",
          classification: "not_committed",
          message: "Content changed",
        }, { status: 409 });
      }
      if (url.pathname.endsWith("/list")) {
        return Response.json({ entries: [], next_after: null });
      }
      if (url.pathname.endsWith("/read-text")) {
        return Response.json({
          entry: entry("2", "alpha", last_mutation_id),
          text: "new",
          truncated: false,
        });
      }
      return Response.json(
        entry(
          id,
          "alpha",
          id === "1" ? "1" : last_mutation_id,
          id === "1" ? "folder" : "file",
        ),
      );
    }),
  );
  const view = render(DrivePage, { workspaceId: "alpha", nodeId: "2" });
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Rename / move" })).toBeDefined()
  );
  await fireEvent.click(view.getByRole("button", { name: "Rename / move" }));
  await fireEvent.input(view.getByLabelText("Name"), {
    target: { value: "my-name.txt" },
  });
  last_mutation_id = "2";
  await fireEvent.click(
    view.getByRole("button", { name: "Refresh" }),
  );
  await waitFor(() =>
    expect(
      view.getByRole("button", { name: "Refresh" }).hasAttribute(
        "disabled",
      ),
    ).toBe(false)
  );
  expect(view.container.textContent).toContain("Changes by another writer will not be overwritten.");
  await fireEvent.click(
    view.getByRole("button", { name: "Apply rename / move" }),
  );
  await waitFor(() => expect(mutations).toHaveLength(1));
  expect(mutations[0].mutation).toMatchObject({
    operation: "relocate",
    expected_mutation_id: "1",
    name: "my-name.txt",
    id: { workspace_id: "alpha", node_id: "2" },
  });
  await waitFor(() => expect(view.container.textContent).toContain("conflict"));
  expect((view.getByLabelText("Name") as HTMLInputElement).value).toBe(
    "my-name.txt",
  );
});

test("Folder picker blocks failed reads, filters files and self, and follows paged destinations", async () => {
  let failList = true;
  const mutations: DriveMutationRequest[] = [];
  const folder = (id: string) => ({
    ...entry(id, "alpha", "7", "folder"),
    name: `Folder ${id}`,
  });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const id = url.searchParams.get("id") ?? "1";
      if (url.pathname.endsWith("/mutate")) {
        mutations.push(JSON.parse(String(init?.body)));
        return Response.json({
          code: "conflict",
          classification: "not_committed",
          message: "Content changed",
        }, { status: 409 });
      }
      if (url.pathname.endsWith("/list")) {
        if (id !== "1") return Response.json({ entries: [], next_after: null });
        if (failList) {
          return Response.json(
            { code: "unavailable", message: "Unavailable" },
            { status: 503 },
          );
        }
        return Response.json(
          url.searchParams.has("after")
            ? { entries: [folder("5")], next_after: null }
            : {
              entries: [folder("2"), entry("3"), folder("4")],
              next_after: "4",
            },
        );
      }
      return Response.json(folder(id));
    }),
  );
  const view = render(DrivePage, { workspaceId: "alpha", nodeId: "2" });
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Rename / move" })).toBeDefined()
  );
  await fireEvent.click(view.getByRole("button", { name: "Rename / move" }));
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Retry folders" })).toBeDefined()
  );
  expect(
    view.getByRole("button", { name: "Apply rename / move" }).hasAttribute(
      "disabled",
    ),
  ).toBe(true);
  expect(mutations).toHaveLength(0);
  failList = false;
  await fireEvent.click(view.getByRole("button", { name: "Retry folders" }));
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Folder 4" })).toBeDefined()
  );
  expect(view.queryByRole("button", { name: "Folder 2" })).toBeNull();
  expect(view.queryByRole("button", { name: "note.txt" })).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "More folders" }));
  await waitFor(() =>
    expect(view.getByRole("button", { name: "Folder 5" })).toBeDefined()
  );
  await fireEvent.click(view.getByRole("button", { name: "Folder 5" }));
  await waitFor(() =>
    expect(
      view.getByRole("button", { name: "Apply rename / move" }).hasAttribute(
        "disabled",
      ),
    ).toBe(false)
  );
  await fireEvent.click(
    view.getByRole("button", { name: "Apply rename / move" }),
  );
  await waitFor(() => expect(mutations).toHaveLength(1));
  expect(mutations[0].mutation).toMatchObject({
    operation: "relocate",
    expected_mutation_id: "7",
    id: { workspace_id: "alpha", node_id: "2" },
    parent: { workspace_id: "alpha", node_id: "5" },
  });
});
