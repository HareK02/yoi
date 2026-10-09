import type { DriveEntry } from "#lib/generated/drive-api.ts";

export { DRIVE_TEXT_MAX_BYTES } from "#lib/workspace/drive/api.ts";
const RASTER_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);
export function previewKind(
  entry: DriveEntry,
): "folder" | "markdown" | "text" | "image" | "download" {
  if (entry.kind === "folder") return "folder";
  const media = (entry.content_type ?? "").split(";")[0].trim().toLowerCase();
  if (RASTER_TYPES.has(media)) return "image";
  // Never infer a preview from an extension, including .md files declared as HTML/SVG.
  if (media === "text/markdown") return "markdown";
  if (media === "text/plain") return "text";
  return "download";
}
export function formatBytes(size: number | null): string {
  if (size === null) return "Folder";
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KiB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}
