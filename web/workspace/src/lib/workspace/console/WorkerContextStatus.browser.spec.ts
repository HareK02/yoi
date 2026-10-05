// @vitest-environment happy-dom

import { cleanup, render } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import WorkerContextStatus from "./WorkerContextStatus.svelte";

afterEach(cleanup);

test("the three-dot Details control invokes the existing toggle and reflects its state", async () => {
  let toggles = 0;
  const view = render(WorkerContextStatus, {
    metadata: null,
    detailsOpen: false,
    onToggleDetails: () => { toggles++; },
  });
  const button = view.getByRole("button", { name: "Details" });
  expect(button.querySelectorAll("circle")).toHaveLength(3);
  expect(button.textContent?.trim()).toBe("");
  expect(button.getAttribute("aria-expanded")).toBe("false");
  button.click();
  expect(toggles).toBe(1);
  await view.rerender({ detailsOpen: true });
  expect(button.getAttribute("aria-expanded")).toBe("true");
});

test("renders resolved model, reasoning, and measured context accessibly", () => {
  const view = render(WorkerContextStatus, {
    metadata: {
      model: "gpt-6-astra",
      reasoning: { kind: "effort", effort: "high" },
      contextWindow: 272_000,
      contextTokens: 142_000,
      contextSource: "measured",
    },
  });

  const status = view.getByLabelText("Worker model and context");
  expect(status.textContent).toContain("gpt-6-astra · high");
  expect(status.textContent).toContain("142.0k / 272.0k (52%)");
});

test("renders honest unavailable state instead of a fabricated zero percent", async () => {
  const view = render(WorkerContextStatus, { metadata: null });
  expect(view.getByText("Model unavailable")).not.toBeNull();
  expect(view.getByText("unavailable")).not.toBeNull();
  expect(view.container.textContent).not.toContain("0%");

  await view.rerender({
    metadata: {
      model: "claude",
      reasoning: { kind: "budget_tokens", budget_tokens: 8_192 },
      contextWindow: 64_000,
      contextTokens: 12_000,
      contextSource: "estimated",
    },
  });
  expect(view.container.textContent).toContain("claude · 8.2k token budget");
  expect(view.container.textContent).toContain("~12.0k / 64.0k (19%)");

  await view.rerender({
    metadata: {
      model: "claude",
      reasoning: { kind: "budget_tokens", budget_tokens: -1 },
      contextWindow: 64_000,
      contextTokens: 12_000,
      contextSource: "estimated",
    },
  });
  expect(view.container.textContent).toContain("claude · dynamic token budget");
});
