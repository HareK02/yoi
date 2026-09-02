import {
  acceptedAttachmentMediaType,
  MAX_UPLOADED_FILE_BYTES,
  validateAttachmentFile,
} from "./composer-attachments.ts";

declare const Deno: { test(name: string, fn: () => void): void };

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

Deno.test("attachment validation accepts bounded text and image files", () => {
  assert(acceptedAttachmentMediaType("text/plain"), "text must be accepted");
  assert(acceptedAttachmentMediaType("image/png"), "png must be accepted");
  assert(
    !acceptedAttachmentMediaType("application/x-executable"),
    "executables must be rejected",
  );
  const valid = { name: "notes.md", type: "text/markdown", size: 32 } as File;
  assert(validateAttachmentFile(valid) === null, "bounded text should pass");
});

Deno.test("attachment validation rejects over-limit files", () => {
  const tooLarge = {
    name: "large.txt",
    type: "text/plain",
    size: MAX_UPLOADED_FILE_BYTES + 1,
  } as File;
  assert(validateAttachmentFile(tooLarge)?.includes("10 MiB"), "limit should be explicit");
});
