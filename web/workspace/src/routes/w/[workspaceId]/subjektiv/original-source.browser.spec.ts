// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import OriginalSource from "#lib/workspace/memory/OriginalSource.svelte";

afterEach(cleanup);
test("a retained navigation option does not attest original bytes or imply deletion", () => {
  render(OriginalSource, {
    source: {
      state: "unavailable",
      history_href: "/w/workspace/workers/W-1/console",
    },
  });
  expect(screen.getByText("原資料の本文は未確認です。")).not.toBeNull();
  expect(
    screen.getByRole("link", { name: "元の会話を確認する →" }).getAttribute(
      "href",
    ),
  ).toBe("/w/workspace/workers/W-1/console");
  expect(screen.queryByText("元の会話は削除済み")).toBeNull();
  expect(screen.queryByText(/原資料は取得不能/)).toBeNull();
});
