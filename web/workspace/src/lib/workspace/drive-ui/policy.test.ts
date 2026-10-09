import { markdownLinkTarget } from "../markdown/link-policy.ts";
import { formatBytes, previewKind } from "./preview-policy.ts";
import type { DriveEntry } from "#lib/generated/drive-api.ts";
declare const Deno: { test(name: string, fn: () => void): void };

Deno.test("Markdown links admit explicit same Workspace navigation and credential-free external URLs only", () => {
  const accepted = [
    "/w/ws/drive/9007199254740993",
    "/w/ws/tickets/T-1-title",
    "/w/ws/objectives/O-2",
  ];
  for (const href of accepted) {
    if (markdownLinkTarget(href, "ws")?.external !== false) {
      throw new Error(href);
    }
  }
  for (
    const href of [
      "//evil.test",
      "/api/w/ws/drive/download",
      "/w/other/drive/1",
      "/w/ws/drive/0",
      "/w/ws/drive/1?token=secret",
      "/w/ws/drive/%2fetc",
      "/w/ws/drive/..",
      "javascript:alert(1)",
      "data:text/html,test",
      "https://user:secret@evil.test",
      "file:///etc/passwd",
    ]
  ) {
    if (markdownLinkTarget(href, "ws") !== null) throw new Error(href);
  }
  if (!markdownLinkTarget("https://example.test/docs", "ws")?.external) {
    throw new Error("external link missing");
  }
});
Deno.test("Drive preview never infers executable HTML or SVG from a Markdown filename", () => {
  const entry = { name: "file.md", kind: "file" } as DriveEntry;
  for (
    const content_type of [
      "text/html",
      "image/svg+xml",
      "application/xhtml+xml",
      "application/octet-stream",
      "application/javascript",
    ]
  ) {
    if (previewKind({ ...entry, content_type }) !== "download") {
      throw new Error(content_type);
    }
  }
  for (
    const content_type of ["image/png", "image/jpeg", "image/gif", "image/webp"]
  ) {
    if (previewKind({ ...entry, content_type }) !== "image") {
      throw new Error(content_type);
    }
  }
  if (previewKind({ ...entry, content_type: "text/markdown" }) !== "markdown") {
    throw new Error("Markdown not supported");
  }
  if (formatBytes(0) !== "0 B") {
    throw new Error("empty file confused with folder");
  }
});
