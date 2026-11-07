// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import SubjectsNavSection from "../sidebar/SubjectsNavSection.svelte";
import MemoryNavSection from "../sidebar/MemoryNavSection.svelte";

afterEach(cleanup);
test("Subjects is a direct resource link, distinct from Memory staging, and follows route changes", async () => {
  const view = render(SubjectsNavSection, {
    workspaceId: "w/s",
    currentPath: "/w/w%2Fs/subjektiv/a/chat",
  });
  render(MemoryNavSection, {
    workspaceId: "w/s",
    currentPath: "/w/w%2Fs/subjektiv/a/chat",
  });
  const subjects = screen.getByRole("link", { name: "Subjects" });
  expect(subjects.getAttribute("href")).toBe("/w/w%2Fs/subjektiv");
  expect(subjects.getAttribute("aria-current")).toBe("page");
  expect(subjects.closest("section")?.querySelector("h2")).toBeNull();
  expect(screen.getByRole("link", { name: "Staging" }).getAttribute("href"))
    .toBe("/w/w%2Fs/memory/staging");
  expect(
    screen.getByRole("link", { name: "Staging" }).getAttribute("aria-current"),
  ).toBeNull();
  await view.rerender({
    workspaceId: "w/s",
    currentPath: "/w/w%2Fs/subjektiv-other",
  });
  expect(subjects.getAttribute("aria-current")).toBeNull();
});
test("Memory staging selection never activates the Subject resource", () => {
  render(SubjectsNavSection, {
    workspaceId: "w",
    currentPath: "/w/w/memory/staging",
  });
  render(MemoryNavSection, {
    workspaceId: "w",
    currentPath: "/w/w/memory/staging",
  });
  expect(
    screen.getByRole("link", { name: "Staging" }).getAttribute("aria-current"),
  ).toBe("page");
  expect(
    screen.getByRole("link", { name: "Subjects" }).getAttribute("aria-current"),
  ).toBeNull();
});
