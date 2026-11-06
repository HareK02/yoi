// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import FailureNotice from "./FailureNotice.svelte";

afterEach(cleanup);
test("failure cause is an alert before optional details; server text cannot create markup or network links", () => {
  const explanation =
    "Subject session.record is denied; Session registration and Worker creation could not proceed. <img src='/secret' onerror='alert(1)'>";
  render(FailureNotice, {
    failure: {
      explanation,
      supplements: ["Other operations remain available."],
      compensation: ["Cleanup could not be confirmed."],
      details: [{ label: "Support correlation", value: "correlation-1" }],
    },
  });
  const alert = screen.getByRole("alert");
  expect(alert.textContent).toContain(explanation);
  expect(alert.querySelector("img")).toBeNull();
  expect(alert.querySelector("a")).toBeNull();
  const details = alert.querySelector("details")!;
  expect(details.open).toBe(false);
  expect(details.textContent).not.toContain("session.record");
  expect(screen.getByText("Cleanup needs attention")).toBeTruthy();
  expect(screen.getByText("Additional information")).toBeTruthy();
  expect(alert.getAttribute("aria-atomic")).toBe("true");
});
