// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import type { TicketEventDetail } from "#lib/generated/ticket-api.ts";
import TicketPost from "./TicketPost.svelte";

afterEach(cleanup);
function post(overrides: Partial<TicketEventDetail> = {}): TicketEventDetail {
  return { sequence: 0, event_ref: "post-1", kind: "comment", is_initial: true,
    author: "Mika", at: "2026-10-12T09:00:00Z", body: "Original request",
    editable: true, body_truncated: false, body_digest: "a".repeat(64), edited_at: null, edited_by: null,
    attributes: {}, references: [], ...overrides };
}

test.each([true, false])("initial=%s has the same author, timestamp and body-only edit action", async (is_initial) => {
  const value = post({ is_initial });
  const save = vi.fn(async () => {});
  const load = vi.fn(async () => value);
  const view = render(TicketPost, { post: value, busy: false, load, save });
  expect(view.getByText("Mika")).toBeTruthy();
  expect(view.container.querySelector("time")?.dateTime).toBe(value.at);
  await fireEvent.click(view.getByRole("button", { name: "Edit post" }));
  expect(view.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", value.body);
  expect(view.queryByRole("textbox", { name: "Title" })).toBeNull();
  await fireEvent.input(view.getByRole("textbox", { name: "Post body" }), { target: { value: "Revised request" } });
  await fireEvent.click(view.getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(save).toHaveBeenCalledWith(value, "Revised request"));
  expect(load).not.toHaveBeenCalled();
});

test("non-editable and unknown-author posts do not infer authorship or editing permission", () => {
  const value = post({ editable: false, author: null, at: null });
  const view = render(TicketPost, { post: value, busy: false, load: async () => value, save: async () => {} });
  expect(view.getByText("Unknown author")).toBeTruthy();
  expect(view.getByText("Unknown time")).toBeTruthy();
  expect(view.queryByRole("button", { name: "Edit post" })).toBeNull();
});

test("edited metadata is separate from the preserved original author and time", () => {
  const value = post({ edited_at: "2026-10-13T11:30:00Z", edited_by: "Rin" });
  const view = render(TicketPost, { post: value, busy: false, load: async () => value, save: async () => {} });
  expect(view.getByText("Mika")).toBeTruthy();
  expect(view.container.querySelector(".ticket-post-edited")?.textContent).toContain("by Rin");
  expect(Array.from(view.container.querySelectorAll("time")).map((time) => time.dateTime)).toEqual([value.at, value.edited_at]);
});

test.each(["close", "state_changed", "assignment_answer", "plan", "decision", "implementation_report"])("immutable %s records never offer post editing", (kind) => {
  const value = post({ kind, is_initial: false });
  const view = render(TicketPost, { post: value, busy: false, load: async () => value, save: async () => {} });
  expect(view.queryByRole("button", { name: "Edit post" })).toBeNull();
});

test("truncated post fetches the full body and its observation before editing", async () => {
  const preview = post({ body: "Short preview", body_truncated: true });
  const full = post({ body: "Complete post including the unseen ending", body_digest: "b".repeat(64) });
  const save = vi.fn(async () => {});
  const load = vi.fn(async () => full);
  const view = render(TicketPost, { post: preview, busy: false, load, save });
  expect(view.queryByRole("textbox")).toBeNull();
  await fireEvent.click(view.getByRole("button", { name: "Edit post" }));
  await waitFor(() => expect(view.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", full.body));
  await fireEvent.click(view.getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(save).toHaveBeenCalledWith(full, full.body));
  expect(load).toHaveBeenCalledWith(preview);
});

test("a failed or still-truncated full-post read cannot open a partial editor", async () => {
  const preview = post({ body: "Short preview", body_truncated: true });
  const save = vi.fn(async () => {});
  const view = render(TicketPost, { post: preview, busy: false, load: async () => preview, save });
  await fireEvent.click(view.getByRole("button", { name: "Edit post" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("No partial text can be saved"));
  expect(view.queryByRole("textbox")).toBeNull();
  expect(save).not.toHaveBeenCalled();
});

test("conflicts retain the draft and stale digest until explicit discard and reload", async () => {
  const value = post();
  const current = post({ body: "Another editor's update", body_digest: "c".repeat(64) });
  const save = vi.fn(async () => { throw new Error("HTTP (409)"); });
  const load = vi.fn(async () => current);
  const view = render(TicketPost, { post: value, busy: false, load, save });
  await fireEvent.click(view.getByRole("button", { name: "Edit post" }));
  await fireEvent.input(view.getByRole("textbox", { name: "Post body" }), { target: { value: "My draft" } });
  await fireEvent.click(view.getByRole("button", { name: "Save post" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("Post update conflict"));
  expect(view.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", "My draft");
  expect(view.getByRole("button", { name: "Save post" })).toHaveProperty("disabled", true);
  expect(save).toHaveBeenCalledWith(value, "My draft");
  expect(load).not.toHaveBeenCalled();
  await fireEvent.click(view.getByRole("button", { name: "Discard draft and load current post" }));
  await waitFor(() => expect(view.getByRole("textbox", { name: "Post body" })).toHaveProperty("value", current.body));
  expect(view.getByRole("button", { name: "Save post" })).toHaveProperty("disabled", false);
});
