import type { UploadedFileRef } from "$lib/generated/protocol.ts";

export const MAX_UPLOADED_FILE_BYTES = 10 * 1024 * 1024;
export const MAX_FILES_PER_SUBMISSION = 8;

export type AttachmentUploadState = "uploading" | "uploaded" | "failed";

export type ComposerAttachment = {
  id: number;
  file: File;
  uploadPath: string;
  state: AttachmentUploadState;
  progress: number;
  reference: UploadedFileRef | null;
  error: string | null;
  request: XMLHttpRequest | null;
};

export function acceptedAttachmentMediaType(mediaType: string): boolean {
  return mediaType.startsWith("text/") ||
    mediaType === "application/json" ||
    mediaType === "application/pdf" ||
    mediaType === "image/png" ||
    mediaType === "image/jpeg" ||
    mediaType === "image/gif" ||
    mediaType === "image/webp";
}

export function validateAttachmentFile(file: File): string | null {
  if (file.size > MAX_UPLOADED_FILE_BYTES) {
    return `File exceeds the ${MAX_UPLOADED_FILE_BYTES / 1024 / 1024} MiB limit.`;
  }
  if (!acceptedAttachmentMediaType(file.type)) {
    return `Unsupported file type: ${file.type || "unknown"}.`;
  }
  return null;
}

export type AttachmentUploadCallbacks = {
  progress(value: number): void;
  complete(reference: UploadedFileRef): void;
  failed(message: string): void;
};

export function uploadAttachment(
  path: string,
  file: File,
  callbacks: AttachmentUploadCallbacks,
): XMLHttpRequest {
  const request = new XMLHttpRequest();
  const query = new URLSearchParams({
    file_name: file.name,
    media_type: file.type,
  });
  request.open("POST", `${path}?${query.toString()}`);
  request.setRequestHeader("content-type", "application/octet-stream");
  request.upload.addEventListener("progress", (event) => {
    if (event.lengthComputable && event.total > 0) {
      callbacks.progress(Math.min(1, event.loaded / event.total));
    }
  });
  request.addEventListener("load", () => {
    if (request.status < 200 || request.status >= 300) {
      callbacks.failed(`Upload failed (${request.status}).`);
      return;
    }
    try {
      const parsed: unknown = JSON.parse(request.responseText);
      if (!isUploadedFileResponse(parsed)) {
        callbacks.failed("Upload returned an invalid attachment reference.");
        return;
      }
      callbacks.complete(parsed.file);
    } catch {
      callbacks.failed("Upload returned an invalid response.");
    }
  });
  request.addEventListener("error", () => callbacks.failed("Upload failed."));
  request.addEventListener("abort", () => callbacks.failed("Upload cancelled."));
  request.send(file);
  return request;
}

function isUploadedFileResponse(
  value: unknown,
): value is { file: UploadedFileRef } {
  if (!value || typeof value !== "object" || !("file" in value)) return false;
  const file = value.file;
  return !!file && typeof file === "object" &&
    "artifact_id" in file && typeof file.artifact_id === "string" &&
    "file_name" in file && typeof file.file_name === "string" &&
    "media_type" in file && typeof file.media_type === "string" &&
    "byte_len" in file && typeof file.byte_len === "number" &&
    "sha256" in file && typeof file.sha256 === "string" &&
    "availability" in file && file.availability === "available";
}
