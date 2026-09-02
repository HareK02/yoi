import type { UploadedFileRef } from "$lib/generated/protocol.ts";

export const MAX_UPLOADED_FILE_BYTES = 10 * 1024 * 1024;
export const MAX_FILES_PER_SUBMISSION = 8;

export type AttachmentUploadState = "uploading" | "uploaded" | "failed";

export type AttachmentUploadHandle = { abort(): void };

export type ComposerAttachment = {
  id: number;
  file: File;
  uploadPath: string;
  uploadId: string;
  state: AttachmentUploadState;
  progress: number;
  reference: UploadedFileRef | null;
  error: string | null;
  request: AttachmentUploadHandle | null;
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
  workerPath: string,
  file: File,
  uploadId: string,
  callbacks: AttachmentUploadCallbacks,
): AttachmentUploadHandle {
  let activeRequest: XMLHttpRequest | null = null;
  let aborted = false;
  const handle: AttachmentUploadHandle = {
    abort() {
      aborted = true;
      activeRequest?.abort();
      void fetch(
        `${workerPath}/attachment-uploads/${encodeURIComponent(uploadId)}`,
        { method: "DELETE" },
      ).catch(() => undefined);
    },
  };
  const query = new URLSearchParams({
    file_name: file.name,
    media_type: file.type,
    upload_id: uploadId,
  });
  const grantRequest = new XMLHttpRequest();
  activeRequest = grantRequest;
  grantRequest.open(
    "POST",
    `${workerPath}/attachment-upload-grants?${query.toString()}`,
  );
  grantRequest.addEventListener("load", () => {
    if (aborted) return;
    if (grantRequest.status < 200 || grantRequest.status >= 300) {
      callbacks.failed(`Upload grant failed (${grantRequest.status}).`);
      return;
    }
    let uploadId: string;
    try {
      const parsed: unknown = JSON.parse(grantRequest.responseText);
      if (!isUploadGrantResponse(parsed)) {
        callbacks.failed("Upload grant returned an invalid response.");
        return;
      }
      uploadId = parsed.upload_id;
    } catch {
      callbacks.failed("Upload grant returned an invalid response.");
      return;
    }

    const uploadRequest = new XMLHttpRequest();
    activeRequest = uploadRequest;
    uploadRequest.open(
      "PUT",
      `${workerPath}/attachment-uploads/${encodeURIComponent(uploadId)}`,
    );
    uploadRequest.setRequestHeader("content-type", "application/octet-stream");
    uploadRequest.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable && event.total > 0) {
        callbacks.progress(Math.min(1, event.loaded / event.total));
      }
    });
    uploadRequest.addEventListener("load", () => {
      if (uploadRequest.status < 200 || uploadRequest.status >= 300) {
        callbacks.failed(`Upload failed (${uploadRequest.status}).`);
        return;
      }
      try {
        const parsed: unknown = JSON.parse(uploadRequest.responseText);
        if (!isUploadedFileResponse(parsed)) {
          callbacks.failed("Upload returned an invalid attachment reference.");
          return;
        }
        callbacks.complete(parsed.file);
      } catch {
        callbacks.failed("Upload returned an invalid response.");
      }
    });
    uploadRequest.addEventListener("error", () => callbacks.failed("Upload failed."));
    uploadRequest.addEventListener("abort", () => callbacks.failed("Upload cancelled."));
    uploadRequest.send(file);
  });
  grantRequest.addEventListener("error", () => callbacks.failed("Upload grant failed."));
  grantRequest.addEventListener("abort", () => callbacks.failed("Upload cancelled."));
  grantRequest.send();
  return handle;
}

function isUploadGrantResponse(
  value: unknown,
): value is { upload_id: string; expires_at_ms: number } {
  return !!value && typeof value === "object" &&
    "upload_id" in value && typeof value.upload_id === "string" &&
    "expires_at_ms" in value && typeof value.expires_at_ms === "number";
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
