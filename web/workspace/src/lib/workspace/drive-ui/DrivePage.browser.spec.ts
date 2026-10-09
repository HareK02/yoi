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

test("Refreshing metadata never silently rebases an open rename form onto another writer revision", async () => {
  let revision = "1";
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
          message: "Revision conflict",
        }, { status: 409 });
      }
      if (url.pathname.endsWith("/read-text")) {
        return Response.json({
          entry: entry("2", "alpha", revision),
          text: "new",
          truncated: false,
        });
      }
      return Response.json(
        entry(
          id,
          "alpha",
          id === "1" ? "1" : revision,
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
  revision = "2";
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
  expect(view.container.textContent).toContain("Expected revision 1.");
  await fireEvent.click(
    view.getByRole("button", { name: "Apply rename / move" }),
  );
  await waitFor(() => expect(mutations).toHaveLength(1));
  expect(mutations[0].mutation).toMatchObject({
    operation: "relocate",
    expected_revision: "1",
    name: "my-name.txt",
    id: { workspace_id: "alpha", node_id: "2" },
  });
  await waitFor(() => expect(view.container.textContent).toContain("conflict"));
  expect((view.getByLabelText("Name") as HTMLInputElement).value).toBe(
    "my-name.txt",
  );
});
