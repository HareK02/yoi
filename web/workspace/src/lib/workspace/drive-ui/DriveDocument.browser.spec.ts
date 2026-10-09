// @vitest-environment happy-dom
import { cleanup, render, waitFor } from "@testing-library/svelte";
import { afterEach, expect, test } from "vitest";
import DriveDocument from "./DriveDocument.svelte";
afterEach(cleanup);
test("Drive Markdown never automatically loads external or authenticated images, scripts or cross Workspace resources", async () => {
  const { container } = render(DriveDocument, {
    workspaceId: "ws",
    text: [
      "# Shared",
      "[latest](/w/ws/drive/9007199254740993)",
      "[other](/w/other/drive/2)",
      "[external](https://example.test)",
      "[credentials](https://user:secret@evil.test)",
      "![tracker](https://example.test/pixel.png)",
      "![API](/api/w/ws/drive/download?id=2)",
      "<script>window.pwned=true</script><img src=x onerror=alert(1)>",
    ].join("\n\n"),
  });
  await waitFor(() =>
    expect(container.querySelector("h1")?.textContent).toBe("Shared")
  );
  expect(container.querySelectorAll("img,script,iframe,object,embed"))
    .toHaveLength(0);
  expect(container.querySelectorAll("a")).toHaveLength(2);
  const latest = container.querySelector(
    'a[href="/w/ws/drive/9007199254740993"]',
  );
  expect(latest).not.toBeNull();
  expect(latest?.getAttribute("target")).toBeNull();
  const external = container.querySelector('a[href="https://example.test"]');
  expect(external?.getAttribute("rel")).toContain("noreferrer");
  expect(external?.getAttribute("target")).toBe("_blank");
  expect(container.textContent).toContain("[Image: tracker");
  expect(container.textContent).toContain("[Image: API");
  expect(container.textContent).not.toContain("Image: unnamed");
  expect(container.textContent).toContain("open its Drive URL to view");
});
